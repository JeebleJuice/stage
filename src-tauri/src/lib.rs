use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
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
    title: String,
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

fn hypr_dispatch(dispatcher: &str, argument: &str) -> Result<(), String> {
    let output = Command::new("hyprctl").args(["dispatch", dispatcher, argument]).output()
        .map_err(|error| format!("Could not dispatch {dispatcher}: {error}"))?;
    if output.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&output.stderr).trim().to_owned()) }
}

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
        let class_name = {
            let initial = json_text(client, "initialClass");
            if initial.is_empty() { json_text(client, "class") } else { initial }
        };
        let title = {
            let initial = json_text(client, "initialTitle");
            if initial.is_empty() { json_text(client, "title") } else { initial }
        };
        let monitor_id = json_i64(client, "monitor");
        HyprWindow {
            address: json_text(client, "address"),
            class_name,
            title,
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

fn resource_match_score(resource: &StageResource, window: &HyprWindow) -> Option<i32> {
    let class_rule = resource.match_class.trim().to_lowercase();
    let title_rule = resource.match_title.trim().to_lowercase();
    let window_class = window.class_name.to_lowercase();
    if !title_rule.is_empty() && !window.title.to_lowercase().contains(&title_rule) { return None; }
    let mut score = if class_rule.is_empty() { 0 } else if window_class == class_rule { 100 } else if window_class.contains(&class_rule) || class_rule.contains(&window_class) { 70 } else { return None; };
    if !title_rule.is_empty() { score += 25; }
    let executable = resource.command.first().and_then(|item| Path::new(item).file_name()).and_then(|item| item.to_str()).unwrap_or_default().to_lowercase();
    let process_matches = !executable.is_empty() && process_identity(window.pid).iter().any(|identity| identity == &executable);
    if process_matches { score += 40; }
    if class_rule.is_empty() && !process_matches { None } else { Some(score) }
}

fn best_window_match(resource: &StageResource, windows: &[HyprWindow], used: &HashSet<String>) -> Option<HyprWindow> {
    windows.iter().filter(|window| !used.contains(&window.address))
        .filter_map(|window| resource_match_score(resource, window).map(|score| (score, window)))
        .max_by_key(|(score, _)| *score)
        .map(|(_, window)| window.clone())
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
        "file" | "folder" | "url" => {
            if resource.target.trim().is_empty() { return Err("Target is empty".into()); }
            let mut command = Command::new("xdg-open");
            command.arg(&resource.target);
            command
        }
        "terminal" => {
            if !resource.command.is_empty() && !resource.confirmed {
                return Err("Command has not been reviewed and confirmed".into());
            }
            let mut command = Command::new(terminal_program());
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

fn place_window(resource: &StageResource, window: &HyprWindow) -> Result<(), String> {
    let selector = format!("address:{}", window.address);
    if !resource.placement.monitor.trim().is_empty() {
        hypr_dispatch("moveworkspacetomonitor", &format!("{} {}", resource.placement.workspace, resource.placement.monitor))?;
    }
    hypr_dispatch("movetoworkspacesilent", &format!("{},{}", resource.placement.workspace, selector))?;
    let should_float = resource.placement.mode == "floating";
    if should_float != window.floating { hypr_dispatch("togglefloating", &selector)?; }
    let should_fullscreen = resource.placement.mode == "fullscreen";
    if should_fullscreen != window.fullscreen { hypr_dispatch("fullscreen", &format!("0 {selector}"))?; }
    if resource.placement.pinned != window.pinned { hypr_dispatch("pin", &selector)?; }
    if should_float {
        if let (Some(x), Some(y)) = (resource.placement.x, resource.placement.y) {
            hypr_dispatch("movewindowpixel", &format!("exact {x} {y},{selector}"))?;
        }
        if let (Some(width), Some(height)) = (resource.placement.width, resource.placement.height) {
            hypr_dispatch("resizewindowpixel", &format!("exact {width} {height},{selector}"))?;
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
    parts.into_iter().filter(|part| !part.starts_with('%')).collect()
}

fn desktop_value<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    content.lines().take_while(|line| *line != "[Desktop Action NewWindow]")
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
}

fn application_directories() -> Vec<PathBuf> {
    let mut paths = vec![PathBuf::from("/usr/share/applications"), PathBuf::from("/usr/local/share/applications")];
    if let Some(data) = dirs::data_local_dir() { paths.push(data.join("applications")); }
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
            match_class: window.class_name,
            match_title: String::new(),
            desktop_file: application.as_ref().map(|item| item.desktop_file.clone()),
            icon: application.as_ref().map(|item| item.icon.clone()),
            placement: Placement { workspace: window.workspace.max(1), monitor: window.monitor, mode: if window.fullscreen { "fullscreen".into() } else if window.floating { "floating".into() } else { "tiled".into() }, pinned: window.pinned, x: None, y: None, width: None, height: None },
            confirmed: false,
        }
    }).collect();
    Ok(StageDefinition { id: Uuid::new_v4().to_string(), name: "Captured setup".into(), description: "Draft captured from open Hyprland windows — review launch details before saving.".into(), color: "#82d8ff".into(), notes: "Stage deliberately did not capture command arguments or browser URLs.".into(), focus_workspace: 1, created_at: now.clone(), updated_at: now, resources })
}

#[tauri::command]
fn cancel_restore(operation_id: String, state: State<AppState>) -> Result<(), String> {
    state.cancelled_restores.lock().map_err(|_| "Restoration state is unavailable".to_string())?.insert(operation_id);
    Ok(())
}

#[tauri::command]
fn restore_stage(operation_id: String, stage: StageDefinition, app: tauri::AppHandle, state: State<AppState>) -> Result<RestoreRun, String> {
    let started = Instant::now();
    let started_at = Utc::now().to_rfc3339();
    state.cancelled_restores.lock().map_err(|_| "Restoration state is unavailable".to_string())?.remove(&operation_id);
    emit_progress(&app, &operation_id, "matching", None, "Inspecting open Hyprland windows");
    let mut windows = read_hypr_windows()?.0;
    let mut used = HashSet::new();
    let mut matched: HashMap<String, (HyprWindow, bool)> = HashMap::new();
    let mut failures: HashMap<String, String> = HashMap::new();

    for resource in &stage.resources {
        if restore_cancelled(&state, &operation_id) { break; }
        if resource.kind == "note" { continue; }
        if let Some(window) = best_window_match(resource, &windows, &used) {
            used.insert(window.address.clone());
            matched.insert(resource.id.clone(), (window, true));
            emit_progress(&app, &operation_id, "matching", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "reused".into(), message: "Matched an existing window".into() }), format!("Matched {}", resource.name));
        } else {
            emit_progress(&app, &operation_id, "launching", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "launched".into(), message: "Launch requested; waiting for its window".into() }), format!("Launching {}", resource.name));
            if let Err(error) = launch_resource(resource) {
                failures.insert(resource.id.clone(), error.clone());
                emit_progress(&app, &operation_id, "launching", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: error }), format!("Could not launch {}", resource.name));
            }
        }
    }

    let pending: Vec<&StageResource> = stage.resources.iter().filter(|resource| resource.kind != "note" && !matched.contains_key(&resource.id) && !failures.contains_key(&resource.id)).collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    emit_progress(&app, &operation_id, "waiting", None, "Waiting for newly launched windows");
    while !restore_cancelled(&state, &operation_id) && Instant::now() < deadline && matched.len() + failures.len() < stage.resources.iter().filter(|resource| resource.kind != "note").count() {
        thread::sleep(Duration::from_millis(350));
        if let Ok((latest, _, _)) = read_hypr_windows() { windows = latest; }
        for resource in &pending {
            if matched.contains_key(&resource.id) { continue; }
            if let Some(window) = best_window_match(resource, &windows, &used) {
                used.insert(window.address.clone());
                matched.insert(resource.id.clone(), (window, false));
                emit_progress(&app, &operation_id, "waiting", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "launched".into(), message: "Window appeared; ready to place".into() }), format!("Found {}", resource.name));
            }
        }
    }

    let was_cancelled = restore_cancelled(&state, &operation_id);
    let mut items = Vec::new();
    for resource in &stage.resources {
        if resource.kind == "note" {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "skipped".into(), message: "Resumption note is available in Stage".into() });
        } else if was_cancelled && !failures.contains_key(&resource.id) {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "skipped".into(), message: "Restoration was cancelled".into() });
        } else if let Some(message) = failures.get(&resource.id) {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: message.clone() });
        } else if let Some((window, reused)) = matched.get(&resource.id) {
            emit_progress(&app, &operation_id, "placing", Some(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: if *reused { "reused".into() } else { "launched".into() }, message: format!("Moving to workspace {}", resource.placement.workspace) }), format!("Placing {}", resource.name));
            match place_window(resource, window) {
                Ok(()) => items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: if *reused { "reused".into() } else { "placed".into() }, message: format!("{} on workspace {}", if *reused { "Reused existing window" } else { "Launched and placed" }, resource.placement.workspace) }),
                Err(error) => items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: format!("Window found, but placement failed: {error}") }),
            }
        } else {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: "No matching window appeared within 10 seconds".into() });
        }
        if let Some(item) = items.last().cloned() { emit_progress(&app, &operation_id, "placing", Some(item), format!("Handled {}", resource.name)); }
    }
    if !was_cancelled { let _ = hypr_dispatch("workspace", &stage.focus_workspace.to_string()); }
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
        .invoke_handler(tauri::generate_handler![list_stages, save_stage, delete_stage, discover_apps, system_snapshot, capture_current, restore_stage, cancel_restore, list_runs])
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
    }

    #[test]
    fn scores_class_and_title_matches() {
        let resource = StageResource { id: "r".into(), kind: "app".into(), name: "Browser".into(), target: String::new(), command: vec!["firefox".into()], working_directory: String::new(), match_class: "firefox".into(), match_title: "Docs".into(), desktop_file: None, icon: None, placement: Placement { workspace: 1, monitor: String::new(), mode: "tiled".into(), pinned: false, x: None, y: None, width: None, height: None }, confirmed: true };
        let window = HyprWindow { address: "0x1".into(), class_name: "firefox".into(), title: "Rust Docs".into(), workspace: 1, monitor: "DP-1".into(), pid: 1, floating: false, fullscreen: false, pinned: false };
        assert_eq!(resource_match_score(&resource, &window), Some(125));
        assert_eq!(resource_match_score(&StageResource { match_title: "Mail".into(), ..resource.clone() }, &window), None);
        assert_eq!(resource_match_score(&StageResource { match_class: "kitty".into(), ..resource }, &window), None);
    }
}
