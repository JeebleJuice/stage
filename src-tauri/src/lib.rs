use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    io::{Read, Write},
    net::{TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager, State};
use uuid::Uuid;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Placement {
    workspace: i64,
    monitor: String,
    mode: String,
    pinned: bool,
    x: Option<i64>,
    y: Option<i64>,
    width: Option<i64>,
    height: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageResource {
    id: String,
    kind: String,
    name: String,
    target: String,
    command: Vec<String>,
    working_directory: String,
    match_class: String,
    match_title: String,
    desktop_file: Option<String>,
    icon: Option<String>,
    #[serde(default)]
    wait_for_url: Option<String>,
    #[serde(default)]
    pre_launch: Vec<Vec<String>>,
    placement: Placement,
    confirmed: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageDefinition {
    id: String,
    name: String,
    description: String,
    color: String,
    notes: String,
    #[serde(default)]
    clear_workspaces: bool,
    focus_workspace: i64,
    created_at: String,
    updated_at: String,
    resources: Vec<StageResource>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstalledApp {
    name: String,
    description: String,
    exec: Vec<String>,
    icon: String,
    desktop_file: String,
    wm_class: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HyprWindow {
    address: String,
    class_name: String,
    #[serde(skip_serializing)]
    initial_class: String,
    title: String,
    #[serde(skip_serializing)]
    initial_title: String,
    workspace: i64,
    monitor: String,
    pid: i64,
    floating: bool,
    fullscreen: bool,
    pinned: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemSnapshot {
    connected: bool,
    hyprland_version: String,
    monitors: Vec<String>,
    workspaces: Vec<i64>,
    windows: Vec<HyprWindow>,
    message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreItem {
    resource_id: String,
    name: String,
    status: String,
    message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreRun {
    id: String,
    stage_id: String,
    stage_name: String,
    started_at: String,
    duration_ms: u128,
    status: String,
    items: Vec<RestoreItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreProgress {
    operation_id: String,
    phase: String,
    item: Option<RestoreItem>,
    message: String,
}

struct AppState {
    db_path: PathBuf,
    cancelled_restores: Mutex<HashSet<String>>,
}

fn initialise_database(path: &Path) -> Result<(), String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS stages (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            document TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS restore_runs (
            id TEXT PRIMARY KEY,
            started_at TEXT NOT NULL,
            document TEXT NOT NULL
        );"
    ).map_err(|error| error.to_string())
}

fn hypr_json(topic: &str) -> Result<Value, String> {
    let output = Command::new("hyprctl").args(["-j", topic]).output()
        .map_err(|error| format!("Could not start hyprctl: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("Invalid hyprctl JSON for {topic}: {error}"))
}

fn hypr_dispatch(expression: &str) -> Result<(), String> {
    let output = Command::new("hyprctl").args(["dispatch", expression]).output()
        .map_err(|error| format!("Could not dispatch Hyprland action: {error}"))?;
    if output.status.success() { return Ok(()); }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if stdout.is_empty() { stderr } else if stderr.is_empty() { stdout } else { format!("{stdout}\n{stderr}") })
}

fn lua_string(value: &str) -> String { format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")) }

fn json_i64(value: &Value, field: &str) -> i64 { value.get(field).and_then(Value::as_i64).unwrap_or_default() }
fn json_text(value: &Value, field: &str) -> String { value.get(field).and_then(Value::as_str).unwrap_or_default().to_owned() }

fn read_hypr_windows() -> Result<(Vec<HyprWindow>, Vec<String>, Vec<i64>), String> {
    let monitor_json = hypr_json("monitors")?;
    let mut monitor_by_id = HashMap::new();
    let mut monitors = Vec::new();
    for monitor in monitor_json.as_array().into_iter().flatten() {
        let name = json_text(monitor, "name");
        monitor_by_id.insert(json_i64(monitor, "id"), name.clone());
        if !name.is_empty() { monitors.push(name); }
    }
    let workspace_json = hypr_json("workspaces")?;
    let mut workspaces: Vec<i64> = workspace_json.as_array().into_iter().flatten().map(|item| json_i64(item, "id")).filter(|id| *id > 0).collect();
    workspaces.sort_unstable();
    workspaces.dedup();

    let clients = hypr_json("clients")?;
    let windows = clients.as_array().into_iter().flatten().map(|client| {
        let class_name = json_text(client, "class");
        let initial_class = json_text(client, "initialClass");
        let title = json_text(client, "title");
        let initial_title = json_text(client, "initialTitle");
        let monitor_id = json_i64(client, "monitor");
        HyprWindow {
            address: json_text(client, "address"),
            class_name,
            initial_class,
            title,
            initial_title,
            workspace: client.get("workspace").map(|workspace| json_i64(workspace, "id")).unwrap_or_default(),
            monitor: monitor_by_id.get(&monitor_id).cloned().unwrap_or_default(),
            pid: json_i64(client, "pid"),
            floating: client.get("floating").and_then(Value::as_bool).unwrap_or(false),
            fullscreen: json_i64(client, "fullscreen") > 0,
            pinned: client.get("pinned").and_then(Value::as_bool).unwrap_or(false),
        }
    }).collect();
    Ok((windows, monitors, workspaces))
}

fn process_identity(pid: i64) -> Vec<String> {
    let mut identities = Vec::new();
    let mut current = pid;
    for _ in 0..5 {
        if current <= 1 { break; }
        if let Ok(executable) = fs::read_link(format!("/proc/{current}/exe")) {
            if let Some(name) = executable.file_name().and_then(|value| value.to_str()) {
                identities.push(name.to_lowercase());
            }
        }
        if let Ok(command_line) = fs::read(format!("/proc/{current}/cmdline")) {
            if let Some(first) = command_line.split(|byte| *byte == 0).next().filter(|part| !part.is_empty()) {
                if let Some(name) = Path::new(&String::from_utf8_lossy(first).into_owned()).file_name().and_then(|value| value.to_str()) {
                    identities.push(name.to_lowercase());
                }
            }
        }
        let Ok(stat) = fs::read_to_string(format!("/proc/{current}/stat")) else { break; };
        let Some(after_name) = stat.rsplit_once(')').map(|(_, rest)| rest.trim()) else { break; };
        current = after_name.split_whitespace().nth(1).and_then(|value| value.parse().ok()).unwrap_or(0);
    }
    identities.sort();
    identities.dedup();
    identities
}

fn process_parent(pid: i64) -> Option<i64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(')')?.1.trim();
    after_name.split_whitespace().nth(1)?.parse().ok()
}

fn is_descendant(mut pid: i64, ancestor: i64) -> bool {
    for _ in 0..32 {
        let Some(parent) = process_parent(pid) else { return false; };
        if parent == ancestor { return true; }
        if parent <= 1 || parent == pid { return false; }
        pid = parent;
    }
    false
}

fn terminal_command_active(window_pid: i64, command: &[String]) -> bool {
    if command.is_empty() { return true; }
    let expected = command.iter().filter(|argument| argument.as_str() != "--").map(|argument| argument.to_lowercase()).collect::<Vec<_>>();
    let Ok(processes) = fs::read_dir("/proc") else { return false; };
    processes.flatten().filter_map(|entry| entry.file_name().to_string_lossy().parse::<i64>().ok()).any(|pid| {
        if pid == window_pid || !is_descendant(pid, window_pid) { return false; }
        fs::read(format!("/proc/{pid}/cmdline")).ok().is_some_and(|bytes| {
            let actual = String::from_utf8_lossy(&bytes).replace('\0', " ").to_lowercase();
            expected.iter().all(|argument| actual.contains(argument))
        })
    })
}

fn resource_match_score(resource: &StageResource, window: &HyprWindow) -> Option<i32> {
    let class_rule = resource.match_class.trim().to_lowercase();
    let title_rule = resource.match_title.trim().to_lowercase();
    let window_class = window.class_name.to_lowercase();
    let initial_class = window.initial_class.to_lowercase();
    if !title_rule.is_empty() && !window.title.to_lowercase().contains(&title_rule) && !window.initial_title.to_lowercase().contains(&title_rule) { return None; }
    let executable = resource.command.first().and_then(|item| Path::new(item).file_name()).and_then(|item| item.to_str()).unwrap_or_default().to_lowercase();
    let process_matches = !executable.is_empty() && process_identity(window.pid).iter().any(|identity| identity == &executable);
    let class_score = if class_rule.is_empty() { None } else if window_class == class_rule || initial_class == class_rule { Some(100) } else if window_class.contains(&class_rule) || class_rule.contains(&window_class) || initial_class.contains(&class_rule) || class_rule.contains(&initial_class) { Some(70) } else { None };
    if class_score.is_none() && !process_matches { return None; }
    let mut score = class_score.unwrap_or_default();
    if !title_rule.is_empty() { score += 25; }
    if process_matches { score += 40; }
    Some(score)
}

fn best_window_match(resource: &StageResource, windows: &[HyprWindow], used: &HashSet<String>) -> Option<HyprWindow> {
    windows.iter().filter(|window| !used.contains(&window.address))
        .filter(|window| resource.kind != "terminal" || terminal_command_active(window.pid, &resource.command))
        .filter_map(|window| resource_match_score(resource, window).map(|score| (score, window)))
        .max_by_key(|(score, _)| *score)
        .map(|(_, window)| window.clone())
}

fn evacuation_destinations(windows: &[HyprWindow], targets: &[i64]) -> HashMap<i64, i64> {
    let occupied: HashSet<i64> = windows.iter().map(|window| window.workspace).filter(|workspace| *workspace > 0).collect();
    let mut ordered = targets.iter().copied().filter(|workspace| *workspace > 0).collect::<Vec<_>>();
    ordered.sort_unstable();
    ordered.dedup();
    if ordered.is_empty() { return HashMap::new(); }
    let mut base = 4;
    while (base..base + ordered.len() as i64).any(|workspace| occupied.contains(&workspace)) { base += 1; }
    ordered.into_iter().enumerate().map(|(offset, source)| (source, base + offset as i64)).collect()
}

fn evacuate_stage_workspaces(stage: &StageDefinition, windows: &[HyprWindow]) -> Result<usize, String> {
    let targets = stage.resources.iter().filter(|resource| resource.kind != "note").map(|resource| resource.placement.workspace).collect::<Vec<_>>();
    let destinations = evacuation_destinations(windows, &targets);
    let mut moved = 0;
    for window in windows {
        let Some(destination) = destinations.get(&window.workspace) else { continue; };
        let selector = format!("address:{}", window.address);
        hypr_dispatch(&format!("hl.dsp.window.move({{ workspace = {}, follow = false, window = {} }})", lua_string(&destination.to_string()), lua_string(&selector)))?;
        moved += 1;
    }
    Ok(moved)
}

fn emit_progress(app: &tauri::AppHandle, operation_id: &str, phase: &str, item: Option<RestoreItem>, message: impl Into<String>) {
    let _ = app.emit("stage://restore-progress", RestoreProgress { operation_id: operation_id.into(), phase: phase.into(), item, message: message.into() });
}

fn restore_cancelled(state: &State<AppState>, operation_id: &str) -> bool {
    state.cancelled_restores.lock().map(|items| items.contains(operation_id)).unwrap_or(false)
}

fn terminal_program() -> String {
    env::var("STAGE_TERMINAL").ok().filter(|value| !value.trim().is_empty())
        .or_else(|| env::var("TERMINAL").ok().filter(|value| !value.trim().is_empty()))
        .unwrap_or_else(|| "kitty".into())
}

fn launch_resource(resource: &StageResource) -> Result<(), String> {
    if resource.kind == "note" { return Ok(()); }
    let mut command = match resource.kind.as_str() {
        "file" | "folder" => {
            if resource.target.trim().is_empty() { return Err("Target is empty".into()); }
            let mut command = Command::new("xdg-open");
            command.arg(&resource.target);
            command
        }
        "url" => {
            if resource.target.trim().is_empty() { return Err("URL is empty".into()); }
            if resource.command.is_empty() {
                let mut command = Command::new("xdg-open");
                command.arg(&resource.target);
                command
            } else {
                let mut command = Command::new(&resource.command[0]);
                command.args(&resource.command[1..]).arg(&resource.target);
                command
            }
        }
        "terminal" => {
            if !resource.command.is_empty() && !resource.confirmed {
                return Err("Command has not been reviewed and confirmed".into());
            }
            let mut command = Command::new(terminal_program());
            if !resource.match_title.trim().is_empty() {
                command.args(["--override", "dynamic_title=no", "--title", resource.match_title.as_str()]);
            }
            if !resource.working_directory.trim().is_empty() {
                command.args(["--directory", resource.working_directory.as_str()]);
            }
            if !resource.command.is_empty() {
                command.arg("--hold").args(&resource.command);
            }
            command
        }
        "app" => {
            if resource.command.is_empty() {
                let desktop_id = resource.desktop_file.as_deref().unwrap_or_default().trim_end_matches(".desktop");
                if desktop_id.is_empty() { return Err("Application has no launch command".into()); }
                let mut command = Command::new("gtk-launch");
                command.arg(desktop_id);
                command
            } else {
                let mut command = Command::new(&resource.command[0]);
                command.args(&resource.command[1..]);
                if !resource.target.trim().is_empty() { command.arg(&resource.target); }
                command
            }
        }
        other => return Err(format!("Unsupported resource kind: {other}")),
    };
    if !resource.working_directory.trim().is_empty() && resource.kind != "terminal" {
        command.current_dir(&resource.working_directory);
    }
    command.spawn().map(|_| ()).map_err(|error| format!("Could not launch {}: {error}", resource.name))
}

fn run_pre_launch(resource: &StageResource) -> Result<(), String> {
    for arguments in &resource.pre_launch {
        let Some(program) = arguments.first() else { continue; };
        let mut command = Command::new(program);
        command.args(&arguments[1..]);
        if !resource.working_directory.trim().is_empty() { command.current_dir(&resource.working_directory); }
        let output = command.output().map_err(|error| format!("Could not run prerequisite `{}`: {error}", arguments.join(" ")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let detail = if !stderr.is_empty() { stderr } else { stdout };
            return Err(format!("Prerequisite `{}` failed{}", arguments.join(" "), if detail.is_empty() { String::new() } else { format!(": {detail}") }));
        }
    }
    Ok(())
}

fn http_endpoint(url: &str) -> Option<(String, u16, String)> {
    let remainder = url.strip_prefix("http://")?;
    let (authority, path) = remainder.split_once('/').map(|(authority, path)| (authority, format!("/{path}"))).unwrap_or((remainder, "/".into()));
    let (host, port) = authority.rsplit_once(':').and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port))).unwrap_or((authority, 80));
    if host.is_empty() { None } else { Some((host.into(), port, path)) }
}

fn url_is_ready(url: &str) -> bool {
    let Some((host, port, path)) = http_endpoint(url) else { return false; };
    let Ok(addresses) = (host.as_str(), port).to_socket_addrs() else { return false; };
    for address in addresses {
        let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(350)) else { continue; };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        if stream.write_all(request.as_bytes()).is_err() { continue; }
        let mut response = [0_u8; 64];
        let Ok(read) = stream.read(&mut response) else { continue; };
        let status_line = String::from_utf8_lossy(&response[..read]);
        let status = status_line.split_whitespace().nth(1).and_then(|value| value.parse::<u16>().ok());
        if status.is_some_and(|code| code < 500) { return true; }
    }
    false
}

fn wait_until_ready(url: &str, timeout: Duration, cancelled: impl Fn() -> bool) -> Result<(), String> {
    if http_endpoint(url).is_none() { return Err(format!("Unsupported readiness URL: {url}")); }
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cancelled() { return Err("Restoration was cancelled".into()); }
        if url_is_ready(url) { return Ok(()); }
        thread::sleep(Duration::from_millis(500));
    }
    Err(format!("Timed out waiting for {url}"))
}

fn place_window(resource: &StageResource, window: &HyprWindow) -> Result<(), String> {
    let selector = format!("address:{}", window.address);
    if !resource.placement.monitor.trim().is_empty() {
        hypr_dispatch(&format!("hl.dsp.workspace.move({{ workspace = {}, monitor = {} }})", lua_string(&resource.placement.workspace.to_string()), lua_string(&resource.placement.monitor)))?;
    }
    hypr_dispatch(&format!("hl.dsp.window.move({{ workspace = {}, follow = false, window = {} }})", lua_string(&resource.placement.workspace.to_string()), lua_string(&selector)))?;
    let should_float = resource.placement.mode == "floating";
    if should_float != window.floating {
        hypr_dispatch(&format!("hl.dsp.window.float({{ action = {}, window = {} }})", lua_string(if should_float { "set" } else { "unset" }), lua_string(&selector)))?;
    }
    let should_fullscreen = resource.placement.mode == "fullscreen";
    if should_fullscreen != window.fullscreen {
        hypr_dispatch(&format!("hl.dsp.window.fullscreen({{ mode = \"fullscreen\", action = {}, window = {} }})", lua_string(if should_fullscreen { "set" } else { "unset" }), lua_string(&selector)))?;
    }
    if resource.placement.pinned != window.pinned {
        hypr_dispatch(&format!("hl.dsp.window.pin({{ action = {}, window = {} }})", lua_string(if resource.placement.pinned { "set" } else { "unset" }), lua_string(&selector)))?;
    }
    if should_float {
        if let (Some(x), Some(y)) = (resource.placement.x, resource.placement.y) {
            hypr_dispatch(&format!("hl.dsp.window.move({{ x = {x}, y = {y}, relative = false, window = {} }})", lua_string(&selector)))?;
        }
        if let (Some(width), Some(height)) = (resource.placement.width, resource.placement.height) {
            hypr_dispatch(&format!("hl.dsp.window.resize({{ x = {width}, y = {height}, relative = false, window = {} }})", lua_string(&selector)))?;
        }
    }
    Ok(())
}

fn parse_exec_line(line: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for character in line.chars() {
        if escaped { current.push(character); escaped = false; continue; }
        if character == '\\' { escaped = true; continue; }
        if let Some(active_quote) = quote {
            if character == active_quote { quote = None; } else { current.push(character); }
            continue;
        }
        if character == '"' || character == '\'' { quote = Some(character); }
        else if character.is_whitespace() {
            if !current.is_empty() { parts.push(std::mem::take(&mut current)); }
        } else { current.push(character); }
    }
    if !current.is_empty() { parts.push(current); }
    parts.into_iter().filter(|part| !part.starts_with('%') && !part.starts_with("@@")).collect()
}

fn desktop_value<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content.lines().take_while(|line| *line != "[Desktop Action NewWindow]")
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
}

fn application_directories() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/usr/share/applications"), PathBuf::from("/usr/local/share/applications")];
    if let Some(data) = dirs::data_local_dir() {
        paths.push(data.join("applications"));
        paths.push(data.join("flatpak/exports/share/applications"));
    }
    paths.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    paths
}

#[tauri::command]
fn list_stages(state: State<AppState>) -> Result<Vec<StageDefinition>, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let mut statement = connection.prepare("SELECT document FROM stages ORDER BY updated_at DESC").map_err(|error| error.to_string())?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?;
    rows.map(|row| row.map_err(|error| error.to_string()).and_then(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))).collect()
}

#[tauri::command]
fn save_stage(stage: StageDefinition, state: State<AppState>) -> Result<(), String> {
    let document = serde_json::to_string(&stage).map_err(|error| error.to_string())?;
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    connection.execute(
        "INSERT INTO stages (id,name,updated_at,document) VALUES (?1,?2,?3,?4)
         ON CONFLICT(id) DO UPDATE SET name=excluded.name, updated_at=excluded.updated_at, document=excluded.document",
        params![stage.id, stage.name, stage.updated_at, document]
    ).map(|_| ()).map_err(|error| error.to_string())
}

#[tauri::command]
fn delete_stage(id: String, state: State<AppState>) -> Result<(), String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    connection.execute("DELETE FROM stages WHERE id = ?1", [id]).map(|_| ()).map_err(|error| error.to_string())
}

fn read_installed_apps() -> Vec<InstalledApp> {
    let mut applications = Vec::new();
    let mut seen = HashSet::new();
    for directory in application_directories() {
        let Ok(entries) = fs::read_dir(directory) else { continue; };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("desktop") { continue; }
            let Ok(content) = fs::read_to_string(&path) else { continue; };
            if desktop_value(&content, "Type") != Some("Application") || desktop_value(&content, "NoDisplay") == Some("true") || desktop_value(&content, "Hidden") == Some("true") { continue; }
            let Some(name) = desktop_value(&content, "Name") else { continue; };
            let exec = desktop_value(&content, "Exec").map(parse_exec_line).unwrap_or_default();
            if exec.is_empty() || !seen.insert(name.to_lowercase()) { continue; }
            applications.push(InstalledApp {
                name: name.to_owned(),
                description: desktop_value(&content, "Comment").unwrap_or_default().to_owned(),
                exec,
                icon: desktop_value(&content, "Icon").unwrap_or_default().to_owned(),
                desktop_file: path.file_name().unwrap_or_default().to_string_lossy().into_owned(),
                wm_class: desktop_value(&content, "StartupWMClass").unwrap_or_else(|| path.file_stem().and_then(|value| value.to_str()).unwrap_or_default()).to_owned(),
            });
        }
    }
    applications.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    applications
}

fn humanize_project_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters.next().map(|first| first.to_uppercase().collect::<String>() + characters.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn local_url_in(text: &str) -> Option<String> {
    for prefix in ["http://localhost:", "http://127.0.0.1:"] {
        if let Some(start) = text.find(prefix) {
            let remainder = &text[start + prefix.len()..];
            let port: String = remainder.chars().take_while(char::is_ascii_digit).collect();
            if !port.is_empty() { return Some(format!("{prefix}{port}")); }
        }
    }
    None
}

fn configured_url(project: &Path) -> String {
    for filename in [".env", ".env.local", ".env.example"] {
        let Ok(content) = fs::read_to_string(project.join(filename)) else { continue; };
        for key in ["APP_URL", "NEXT_PUBLIC_APP_URL", "WEB_ORIGIN"] {
            if let Some(value) = content.lines().find_map(|line| line.strip_prefix(&format!("{key}="))) {
                let cleaned = value.trim().trim_matches(['\'', '"']);
                if cleaned.starts_with("http://localhost:") || cleaned.starts_with("http://127.0.0.1:") { return cleaned.trim_end_matches('/').into(); }
            }
        }
        if let Some(port) = content.lines().find_map(|line| line.strip_prefix("PORT=")) {
            let port = port.trim().trim_matches(['\'', '"']);
            if port.chars().all(|character| character.is_ascii_digit()) { return format!("http://localhost:{port}"); }
        }
    }
    for filename in ["vite.config.ts", "vite.config.js", "vite.config.mjs"] {
        let Ok(content) = fs::read_to_string(project.join(filename)) else { continue; };
        if let Some(after_port) = content.split("port:").nth(1) {
            let port: String = after_port.trim_start().chars().take_while(char::is_ascii_digit).collect();
            if !port.is_empty() { return format!("http://localhost:{port}"); }
        }
    }
    fs::read_to_string(project.join("README.md")).ok().and_then(|content| local_url_in(&content)).unwrap_or_else(|| "http://localhost:3000".into())
}

fn project_slug(name: &str) -> String {
    let slug: String = name.to_lowercase().chars().map(|character| if character.is_ascii_alphanumeric() { character } else { '-' }).collect();
    slug.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

fn compose_defines_service(project: &Path, service: &str) -> bool {
    ["compose.yml", "compose.yaml", "docker-compose.yml", "docker-compose.yaml"].iter().any(|filename| {
        fs::read_to_string(project.join(filename)).ok().is_some_and(|content| content.lines().any(|line| {
            let trimmed = line.trim();
            line.len() > trimmed.len() && trimmed == format!("{service}:")
        }))
    })
}

fn terminal_resource(id: &str, name: &str, title: &str, project: &Path, command: Vec<String>) -> StageResource {
    StageResource {
        id: id.into(), kind: "terminal".into(), name: name.into(), target: command.join(" "), command,
        working_directory: project.display().to_string(), match_class: "kitty".into(), match_title: title.into(),
        desktop_file: None, icon: Some("kitty".into()), wait_for_url: None, pre_launch: Vec::new(),
        placement: Placement { workspace: 1, monitor: String::new(), mode: "tiled".into(), pinned: false, x: None, y: None, width: None, height: None },
        confirmed: true,
    }
}

fn development_stages(root: &Path) -> Vec<StageDefinition> {
    let applications = read_installed_apps();
    let default_browser_id = Command::new("xdg-settings").args(["get", "default-web-browser"]).output().ok()
        .filter(|output| output.status.success()).map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned()).unwrap_or_default();
    let browser = applications.iter().find(|app| app.desktop_file == default_browser_id)
        .or_else(|| applications.iter().find(|app| app.name.to_lowercase().contains("zen")))
        .or_else(|| applications.iter().find(|app| app.name.to_lowercase().contains("firefox")));
    let editor = applications.iter().find(|app| app.desktop_file == "com.microsoft.VSCode.desktop")
        .or_else(|| applications.iter().find(|app| app.name == "Visual Studio Code"));
    let Ok(entries) = fs::read_dir(root) else { return Vec::new(); };
    let mut project_directories: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
    project_directories.sort();
    let mut projects = Vec::new();
    let colors = ["#d7ff72", "#82d8ff", "#ffab76", "#c7a5ff", "#ff83ad"];
    for project in project_directories {
        let Ok(package_text) = fs::read_to_string(project.join("package.json")) else { continue; };
        let Ok(package): Result<Value, _> = serde_json::from_str(&package_text) else { continue; };
        let Some(scripts) = package.get("scripts").and_then(Value::as_object) else { continue; };
        if !scripts.contains_key("dev") { continue; }
        let directory_name = project.file_name().and_then(|value| value.to_str()).unwrap_or("Project");
        let name = match directory_name {
            "autostickarena" => "Tribal Auto Sticks".into(),
            _ => humanize_project_name(directory_name),
        };
        let slug = project_slug(directory_name);
        let project_url = configured_url(&project);
        let dev_script = scripts.get("dev").and_then(Value::as_str).unwrap_or_default();
        let mut dev_command = vec!["npm".into(), "run".into(), "dev".into()];
        if dev_script.contains("next dev") {
            if let Some((_, port, _)) = http_endpoint(&project_url) {
                if port != 3000 { dev_command.extend(["--".into(), "--port".into(), port.to_string()]); }
            }
        }
        let mut development = terminal_resource(&format!("{slug}-dev"), "Development server", &format!("Stage · {name} · Dev"), &project, dev_command);
        if scripts.contains_key("db:dev") { development.pre_launch.push(vec!["npm".into(), "run".into(), "db:dev".into()]); }
        if scripts.contains_key("prisma:deploy:local") { development.pre_launch.push(vec!["npm".into(), "run".into(), "prisma:deploy:local".into()]); }
        if scripts.contains_key("db:setup") {
            if compose_defines_service(&project, "db") {
                development.pre_launch.push(vec!["docker".into(), "compose".into(), "up".into(), "-d".into(), "--wait".into(), "db".into()]);
            }
            development.pre_launch.push(vec!["npm".into(), "run".into(), "db:setup".into()]);
        }
        let mut resources = vec![
            development,
            terminal_resource(&format!("{slug}-codex"), "Codex session", &format!("Stage · {name} · Codex"), &project, vec!["codex".into()]),
        ];
        if scripts.contains_key("worker") {
            let mut worker = terminal_resource(&format!("{slug}-worker"), "Background worker", &format!("Stage · {name} · Worker"), &project, vec!["npm".into(), "run".into(), "worker".into()]);
            worker.wait_for_url = Some(project_url.clone());
            resources.push(worker);
        }
        let mut browser_command = browser.map(|app| app.exec.clone()).unwrap_or_default();
        let browser_name = browser.map(|app| app.name.to_lowercase()).unwrap_or_default();
        if ["firefox", "zen", "chromium", "chrome"].iter().any(|name| browser_name.contains(name)) && !browser_command.iter().any(|argument| argument == "--new-window") {
            browser_command.push("--new-window".into());
        }
        resources.push(StageResource {
            id: format!("{slug}-browser"), kind: "url".into(), name: "Application in browser".into(), target: project_url.clone(), command: browser_command,
            working_directory: String::new(), match_class: browser.map(|app| app.wm_class.clone()).unwrap_or_else(|| "zen".into()), match_title: String::new(),
            desktop_file: browser.map(|app| app.desktop_file.clone()), icon: browser.map(|app| app.icon.clone()), wait_for_url: Some(project_url), pre_launch: Vec::new(),
            placement: Placement { workspace: 2, monitor: String::new(), mode: "tiled".into(), pinned: false, x: None, y: None, width: None, height: None }, confirmed: true,
        });
        resources.push(StageResource {
            id: format!("{slug}-editor"), kind: "app".into(), name: "Project editor".into(), target: project.display().to_string(),
            command: editor.map(|app| app.exec.clone()).unwrap_or_else(|| vec!["code".into()]), working_directory: project.display().to_string(),
            match_class: editor.map(|app| app.wm_class.clone()).unwrap_or_else(|| "code".into()), match_title: directory_name.into(),
            desktop_file: editor.map(|app| app.desktop_file.clone()), icon: editor.map(|app| app.icon.clone()), wait_for_url: None, pre_launch: Vec::new(),
            placement: Placement { workspace: 3, monitor: String::new(), mode: "tiled".into(), pinned: false, x: None, y: None, width: None, height: None }, confirmed: true,
        });
        let now = Utc::now().to_rfc3339();
        projects.push(StageDefinition {
            id: format!("{slug}-development"), name: format!("{name} development"),
            description: format!("{} terminals, browser, and project editor", if scripts.contains_key("worker") { 3 } else { 2 }),
            color: colors[projects.len() % colors.len()].into(), notes: format!("Generated from {}/package.json. Review the current task before coding.", project.display()),
            clear_workspaces: true, focus_workspace: 1, created_at: now.clone(), updated_at: now, resources,
        });
    }
    projects.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    projects
}

#[tauri::command]
fn discover_development_stages(root: Option<String>) -> Result<Vec<StageDefinition>, String> {
    let root = root.map(PathBuf::from).or_else(|| dirs::home_dir().map(|home| home.join("Dev"))).ok_or("Home directory is unavailable")?;
    Ok(development_stages(&root))
}

#[tauri::command]
fn discover_apps() -> Result<Vec<InstalledApp>, String> {
    Ok(read_installed_apps())
}

#[tauri::command]
fn system_snapshot() -> Result<SystemSnapshot, String> {
    match read_hypr_windows() {
        Ok((windows, monitors, workspaces)) => {
            let version = hypr_json("version").ok().and_then(|value| value.get("tag").and_then(Value::as_str).map(str::to_owned)).unwrap_or_else(|| "Hyprland".into());
            Ok(SystemSnapshot { connected: true, hyprland_version: version, monitors, workspaces, windows, message: "Connected through hyprctl JSON IPC".into() })
        }
        Err(error) => Ok(SystemSnapshot { connected: false, hyprland_version: "Unavailable".into(), monitors: Vec::new(), workspaces: Vec::new(), windows: Vec::new(), message: error }),
    }
}

#[tauri::command]
fn capture_current() -> Result<StageDefinition, String> {
    let (windows, _, _) = read_hypr_windows()?;
    let applications = read_installed_apps();
    let now = Utc::now().to_rfc3339();
    let resources = windows.into_iter().map(|window| {
        let lower_class = window.class_name.to_lowercase();
        let kind = if ["kitty", "alacritty", "foot", "wezterm", "konsole"].iter().any(|terminal| lower_class.contains(terminal)) { "terminal" } else { "app" };
        let application = (kind == "app").then(|| applications.iter().find(|application| {
            let wm_class = application.wm_class.to_lowercase();
            !wm_class.is_empty() && (wm_class == lower_class || lower_class.contains(&wm_class) || wm_class.contains(&lower_class))
        }).cloned()).flatten();
        StageResource {
            id: Uuid::new_v4().to_string(),
            kind: kind.into(),
            name: application.as_ref().map(|item| item.name.clone()).unwrap_or_else(|| if window.class_name.is_empty() { "Captured window".into() } else { window.class_name.clone() }),
            target: String::new(),
            command: application.as_ref().map(|item| item.exec.clone()).unwrap_or_default(),
            working_directory: String::new(),
            match_class: if window.initial_class.is_empty() { window.class_name } else { window.initial_class },
            match_title: String::new(),
            desktop_file: application.as_ref().map(|item| item.desktop_file.clone()),
            icon: application.as_ref().map(|item| item.icon.clone()),
            wait_for_url: None,
            pre_launch: Vec::new(),
            placement: Placement { workspace: window.workspace.max(1), monitor: window.monitor, mode: if window.fullscreen { "fullscreen".into() } else if window.floating { "floating".into() } else { "tiled".into() }, pinned: window.pinned, x: None, y: None, width: None, height: None },
            confirmed: false,
        }
    }).collect();
    Ok(StageDefinition { id: Uuid::new_v4().to_string(), name: "Captured setup".into(), description: "Draft captured from open Hyprland windows — review launch details before saving.".into(), color: "#82d8ff".into(), notes: "Stage deliberately did not capture command arguments or browser URLs.".into(), clear_workspaces: false, focus_workspace: 1, created_at: now.clone(), updated_at: now, resources })
}

#[tauri::command]
fn cancel_restore(operation_id: String, state: State<AppState>) -> Result<(), String> {
    state.cancelled_restores.lock().map_err(|_| "Restoration state is unavailable".to_string())?.insert(operation_id);
    Ok(())
}

#[tauri::command]
fn close_stage(stage: StageDefinition) -> Result<usize, String> {
    let target_workspaces: HashSet<i64> = stage.resources.iter()
        .filter(|resource| resource.kind != "note" && resource.placement.workspace > 0)
        .map(|resource| resource.placement.workspace)
        .collect();
    let windows = read_hypr_windows()?.0;
    let stage_pid = std::process::id() as i64;
    let targets = windows.into_iter()
        .filter(|window| window.pid != stage_pid && target_workspaces.contains(&window.workspace))
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    let mut closed = 0;
    for window in targets {
        let selector = format!("address:{}", window.address);
        match hypr_dispatch(&format!("hl.dsp.window.close({{ window = {} }})", lua_string(&selector))) {
            Ok(()) => closed += 1,
            Err(error) => errors.push(format!("{}: {error}", window.title)),
        }
    }
    if errors.is_empty() { Ok(closed) } else { Err(format!("Closed {closed} windows; {}", errors.join("; "))) }
}

#[tauri::command]
fn restore_stage(operation_id: String, stage: StageDefinition, app: tauri::AppHandle, state: State<AppState>) -> Result<RestoreRun, String> {
    let started = Instant::now();
    let started_at = Utc::now().to_rfc3339();
    state.cancelled_restores.lock().map_err(|_| "Restoration state is unavailable".to_string())?.remove(&operation_id);
    if stage.clear_workspaces {
        emit_progress(&app, &operation_id, "preparing", None, "Making room on the Stage workspaces");
        let windows = read_hypr_windows()?.0;
        let moved = evacuate_stage_workspaces(&stage, &windows)?;
        emit_progress(&app, &operation_id, "preparing", None, format!("Moved {moved} existing window{} out of the way", if moved == 1 { "" } else { "s" }));
    }
    emit_progress(&app, &operation_id, "matching", None, "Inspecting open Hyprland windows");
    let initial_windows = read_hypr_windows()?.0;
    let baseline_addresses: HashSet<String> = initial_windows.iter().map(|window| window.address.clone()).collect();
    let mut used = HashSet::new();
    let mut readiness: HashMap<String, Result<(), String>> = HashMap::new();
    let mut items = Vec::new();

    for resource in &stage.resources {
        if restore_cancelled(&state, &operation_id) { break; }
        if resource.kind == "note" {
            let item = RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "skipped".into(), message: "Resumption note is available in Stage".into() };
            emit_progress(&app, &operation_id, "placing", Some(item.clone()), format!("Handled {}", resource.name));
            items.push(item);
            continue;
        }

        if !resource.pre_launch.is_empty() {
            emit_progress(&app, &operation_id, "preparing", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "launched".into(), message: "Running project prerequisites".into() }), format!("Preparing {}", resource.name));
            if let Err(error) = run_pre_launch(resource) {
                let item = RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: error };
                emit_progress(&app, &operation_id, "preparing", Some(item.clone()), format!("Could not prepare {}", resource.name));
                items.push(item);
                continue;
            }
        }

        if let Some(url) = resource.wait_for_url.as_deref() {
            emit_progress(&app, &operation_id, "waiting", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "launched".into(), message: format!("Waiting for {url} before launch") }), format!("Waiting before launching {}", resource.name));
            let result = if let Some(cached) = readiness.get(url) { cached.clone() } else {
                let checked = wait_until_ready(url, Duration::from_secs(45), || restore_cancelled(&state, &operation_id));
                readiness.insert(url.into(), checked.clone());
                checked
            };
            if let Err(error) = result {
                if restore_cancelled(&state, &operation_id) { break; }
                let item = RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: error };
                emit_progress(&app, &operation_id, "waiting", Some(item.clone()), format!("Dependency for {} was not ready", resource.name));
                items.push(item);
                continue;
            }
        }

        let windows = read_hypr_windows()?.0;
        let reuse_existing = resource.kind != "url" || !resource.match_title.trim().is_empty();
        let existing = reuse_existing.then(|| best_window_match(resource, &windows, &used)).flatten();
        let (matched, reused) = if let Some(window) = existing {
            emit_progress(&app, &operation_id, "matching", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "reused".into(), message: "Matched an existing window; placing it now".into() }), format!("Matched {}", resource.name));
            (Some(window), true)
        } else {
            emit_progress(&app, &operation_id, "launching", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "launched".into(), message: "Launch requested; waiting for its window".into() }), format!("Launching {}", resource.name));
            if let Err(error) = launch_resource(resource) {
                let item = RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: error };
                emit_progress(&app, &operation_id, "launching", Some(item.clone()), format!("Could not launch {}", resource.name));
                items.push(item);
                continue;
            }
            let deadline = Instant::now() + Duration::from_secs(12);
            let mut found = None;
            while !restore_cancelled(&state, &operation_id) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(250));
                let Ok((latest, _, _)) = read_hypr_windows() else { continue; };
                let eligible: Vec<HyprWindow> = if resource.kind == "url" && resource.match_title.trim().is_empty() {
                    latest.into_iter().filter(|window| !baseline_addresses.contains(&window.address)).collect()
                } else { latest };
                if let Some(window) = best_window_match(resource, &eligible, &used) { found = Some(window); break; }
            }
            (found, false)
        };

        if restore_cancelled(&state, &operation_id) { break; }
        let item = if let Some(window) = matched {
            used.insert(window.address.clone());
            emit_progress(&app, &operation_id, "placing", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: if reused { "reused".into() } else { "launched".into() }, message: format!("Moving to workspace {}", resource.placement.workspace) }), format!("Placing {}", resource.name));
            match place_window(resource, &window) {
                Ok(()) => RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: if reused { "reused".into() } else { "placed".into() }, message: format!("{} on workspace {}", if reused { "Reused existing window" } else { "Launched and placed" }, resource.placement.workspace) },
                Err(error) => RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: format!("Window found, but placement failed: {error}") },
            }
        } else {
            RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: "No matching window appeared within 12 seconds".into() }
        };
        emit_progress(&app, &operation_id, "placing", Some(item.clone()), format!("Handled {}", resource.name));
        items.push(item);
    }

    let was_cancelled = restore_cancelled(&state, &operation_id);
    if was_cancelled {
        let handled: HashSet<String> = items.iter().map(|item| item.resource_id.clone()).collect();
        for resource in stage.resources.iter().filter(|resource| !handled.contains(&resource.id)) {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "skipped".into(), message: "Restoration was cancelled".into() });
        }
    }
    if !was_cancelled { let _ = hypr_dispatch(&format!("hl.dsp.focus({{ workspace = {} }})", lua_string(&stage.focus_workspace.to_string()))); }
    let failed = items.iter().filter(|item| item.status == "failed").count();
    let handled = items.iter().filter(|item| item.status != "skipped").count();
    let status = if was_cancelled { "cancelled" } else if failed == 0 { "success" } else if failed < handled { "partial" } else { "failed" };
    let run = RestoreRun { id: operation_id.clone(), stage_id: stage.id, stage_name: stage.name, started_at, duration_ms: started.elapsed().as_millis(), status: status.into(), items };
    let document = serde_json::to_string(&run).map_err(|error| error.to_string())?;
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    connection.execute("INSERT INTO restore_runs (id,started_at,document) VALUES (?1,?2,?3)", params![run.id, run.started_at, document]).map_err(|error| error.to_string())?;
    state.cancelled_restores.lock().map_err(|_| "Restoration state is unavailable".to_string())?.remove(&operation_id);
    emit_progress(&app, &operation_id, if was_cancelled { "cancelled" } else { "complete" }, None, if was_cancelled { "Restoration cancelled" } else { "Restoration complete" });
    Ok(run)
}

#[tauri::command]
fn list_runs(state: State<AppState>) -> Result<Vec<RestoreRun>, String> {
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    let mut statement = connection.prepare("SELECT document FROM restore_runs ORDER BY started_at DESC LIMIT 100").map_err(|error| error.to_string())?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|error| error.to_string())?;
    rows.map(|row| row.map_err(|error| error.to_string()).and_then(|json| serde_json::from_str(&json).map_err(|error| error.to_string()))).collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("stage.sqlite3");
            initialise_database(&db_path).map_err(std::io::Error::other)?;
            app.manage(AppState { db_path, cancelled_restores: Mutex::new(HashSet::new()) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_stages, save_stage, delete_stage, discover_apps, discover_development_stages, system_snapshot, capture_current, restore_stage, cancel_restore, close_stage, list_runs])
        .run(tauri::generate_context!())
        .expect("error while running Stage");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_desktop_exec_without_shell_or_field_codes() {
        assert_eq!(parse_exec_line("firefox --new-window %U"), vec!["firefox", "--new-window"]);
        assert_eq!(parse_exec_line("code \"folder with spaces\" %F"), vec!["code", "folder with spaces"]);
        assert_eq!(parse_exec_line("flatpak run app.zen @@u %u @@"), vec!["flatpak", "run", "app.zen"]);
    }

    #[test]
    fn scores_class_and_title_matches() {
        let resource = StageResource { id: "r".into(), kind: "app".into(), name: "Browser".into(), target: String::new(), command: vec!["firefox".into()], working_directory: String::new(), match_class: "firefox".into(), match_title: "Docs".into(), desktop_file: None, icon: None, wait_for_url: None, pre_launch: Vec::new(), placement: Placement { workspace: 1, monitor: String::new(), mode: "tiled".into(), pinned: false, x: None, y: None, width: None, height: None }, confirmed: true };
        let window = HyprWindow { address: "0x1".into(), class_name: "firefox".into(), initial_class: "Navigator".into(), title: "Rust Docs".into(), initial_title: "Mozilla Firefox".into(), workspace: 1, monitor: "DP-1".into(), pid: 1, floating: false, fullscreen: false, pinned: false };
        assert_eq!(resource_match_score(&resource, &window), Some(125));
        assert_eq!(resource_match_score(&StageResource { match_title: "Mail".into(), ..resource.clone() }, &window), None);
        assert_eq!(resource_match_score(&StageResource { match_class: "kitty".into(), ..resource }, &window), None);
    }

    #[test]
    fn process_identity_can_recover_from_a_desktop_class_mismatch() {
        let executable = std::env::current_exe().unwrap();
        let resource = StageResource { id: "editor".into(), kind: "app".into(), name: "Editor".into(), target: String::new(), command: vec![executable.display().to_string()], working_directory: String::new(), match_class: "desktop-entry-class".into(), match_title: String::new(), desktop_file: None, icon: None, wait_for_url: None, pre_launch: Vec::new(), placement: Placement { workspace: 3, monitor: String::new(), mode: "tiled".into(), pinned: false, x: None, y: None, width: None, height: None }, confirmed: true };
        let window = HyprWindow { address: "0x2".into(), class_name: "actual-wayland-class".into(), initial_class: "actual-wayland-class".into(), title: "Project".into(), initial_title: "Editor".into(), workspace: 1, monitor: "DP-1".into(), pid: std::process::id() as i64, floating: false, fullscreen: false, pinned: false };
        assert_eq!(resource_match_score(&resource, &window), Some(40));
    }

    #[test]
    fn discovers_dev_projects_and_adds_workers_only_when_declared() {
        let root = std::env::temp_dir().join(format!("stage-projects-{}", Uuid::new_v4()));
        let app = root.join("photo-app");
        let tool = root.join("small-tool");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&tool).unwrap();
        fs::write(app.join("package.json"), r#"{"scripts":{"dev":"next dev","worker":"tsx worker.ts","db:dev":"docker compose up -d","prisma:deploy:local":"prisma migrate deploy","db:setup":"prisma db push"}}"#).unwrap();
        fs::write(app.join("docker-compose.yml"), "services:\n  setup:\n    image: example\n").unwrap();
        fs::write(app.join(".env.example"), "APP_URL=http://localhost:3300\n").unwrap();
        fs::write(tool.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        fs::write(tool.join("vite.config.ts"), "export default { server: { port: 4173 } }").unwrap();

        let stages = development_stages(&root);
        assert_eq!(stages.len(), 2);
        let photo = stages.iter().find(|stage| stage.id == "photo-app-development").unwrap();
        assert!(photo.clear_workspaces);
        assert_eq!(photo.resources.iter().filter(|resource| resource.placement.workspace == 1).count(), 3);
        let development = photo.resources.iter().find(|resource| resource.name == "Development server").unwrap();
        assert_eq!(development.command, vec!["npm", "run", "dev", "--", "--port", "3300"]);
        assert_eq!(development.pre_launch, vec![vec!["npm", "run", "db:dev"], vec!["npm", "run", "prisma:deploy:local"], vec!["npm", "run", "db:setup"]]);
        assert!(!development.pre_launch.iter().any(|command| command.first().is_some_and(|program| program == "docker")));
        assert_eq!(photo.resources.iter().find(|resource| resource.kind == "url").unwrap().target, "http://localhost:3300");
        assert_eq!(photo.resources.iter().find(|resource| resource.name == "Background worker").unwrap().wait_for_url.as_deref(), Some("http://localhost:3300"));
        assert_eq!(photo.resources.iter().find(|resource| resource.kind == "url").unwrap().wait_for_url.as_deref(), Some("http://localhost:3300"));
        let small = stages.iter().find(|stage| stage.id == "small-tool-development").unwrap();
        assert_eq!(small.resources.iter().filter(|resource| resource.placement.workspace == 1).count(), 2);
        assert_eq!(small.resources.iter().find(|resource| resource.kind == "url").unwrap().target, "http://localhost:4173");
        assert!(stages.iter().all(|stage| stage.resources.iter().any(|resource| resource.placement.workspace == 3 && resource.kind == "app")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finds_an_empty_workspace_block_for_evacuated_windows() {
        let window = |address: &str, workspace| HyprWindow { address: address.into(), class_name: String::new(), initial_class: String::new(), title: String::new(), initial_title: String::new(), workspace, monitor: String::new(), pid: 1, floating: false, fullscreen: false, pinned: false };
        let windows = vec![window("1", 1), window("2", 2), window("4", 4), window("6", 6)];
        let destinations = evacuation_destinations(&windows, &[1, 2, 3]);
        assert_eq!(destinations.get(&1), Some(&7));
        assert_eq!(destinations.get(&2), Some(&8));
        assert_eq!(destinations.get(&3), Some(&9));
    }

    #[test]
    fn parses_local_http_readiness_endpoints() {
        assert_eq!(http_endpoint("http://localhost:3300/api/health"), Some(("localhost".into(), 3300, "/api/health".into())));
        assert_eq!(http_endpoint("http://127.0.0.1"), Some(("127.0.0.1".into(), 80, "/".into())));
        assert_eq!(http_endpoint("https://localhost:3000"), None);
    }
}
