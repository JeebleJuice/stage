use chrono::Utc;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};
use tauri::{Manager, State};
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

struct AppState { db_path: PathBuf }

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

fn resource_matches(resource: &StageResource, window: &HyprWindow) -> bool {
    let class_rule = resource.match_class.trim().to_lowercase();
    let title_rule = resource.match_title.trim().to_lowercase();
    let class_matches = class_rule.is_empty() || window.class_name.to_lowercase().contains(&class_rule);
    let title_matches = title_rule.is_empty() || window.title.to_lowercase().contains(&title_rule);
    !class_rule.is_empty() && class_matches && title_matches
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

#[tauri::command]
fn discover_apps() -> Result<Vec<InstalledApp>, String> {
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
    Ok(applications)
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
    let now = Utc::now().to_rfc3339();
    let resources = windows.into_iter().map(|window| {
        let lower_class = window.class_name.to_lowercase();
        let kind = if ["kitty", "alacritty", "foot", "wezterm", "konsole"].iter().any(|terminal| lower_class.contains(terminal)) { "terminal" } else { "app" };
        StageResource {
            id: Uuid::new_v4().to_string(),
            kind: kind.into(),
            name: if window.class_name.is_empty() { "Captured window".into() } else { window.class_name.clone() },
            target: String::new(),
            command: Vec::new(),
            working_directory: String::new(),
            match_class: window.class_name,
            match_title: String::new(),
            desktop_file: None,
            icon: None,
            placement: Placement { workspace: window.workspace.max(1), monitor: window.monitor, mode: if window.fullscreen { "fullscreen".into() } else if window.floating { "floating".into() } else { "tiled".into() }, pinned: window.pinned, x: None, y: None, width: None, height: None },
            confirmed: false,
        }
    }).collect();
    Ok(StageDefinition { id: Uuid::new_v4().to_string(), name: "Captured setup".into(), description: "Draft captured from open Hyprland windows — review launch details before saving.".into(), color: "#82d8ff".into(), notes: "Stage deliberately did not capture command arguments or browser URLs.".into(), focus_workspace: 1, created_at: now.clone(), updated_at: now, resources })
}

#[tauri::command]
fn restore_stage(stage: StageDefinition, state: State<AppState>) -> Result<RestoreRun, String> {
    let started = Instant::now();
    let started_at = Utc::now().to_rfc3339();
    let mut windows = read_hypr_windows()?.0;
    let mut used = HashSet::new();
    let mut matched: HashMap<String, (HyprWindow, bool)> = HashMap::new();
    let mut failures: HashMap<String, String> = HashMap::new();

    for resource in &stage.resources {
        if resource.kind == "note" { continue; }
        if let Some(window) = windows.iter().find(|window| !used.contains(&window.address) && resource_matches(resource, window)).cloned() {
            used.insert(window.address.clone());
            matched.insert(resource.id.clone(), (window, true));
        } else if let Err(error) = launch_resource(resource) { failures.insert(resource.id.clone(), error); }
    }

    let pending: Vec<&StageResource> = stage.resources.iter().filter(|resource| resource.kind != "note" && !matched.contains_key(&resource.id) && !failures.contains_key(&resource.id)).collect();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && matched.len() + failures.len() < stage.resources.iter().filter(|resource| resource.kind != "note").count() {
        thread::sleep(Duration::from_millis(350));
        if let Ok((latest, _, _)) = read_hypr_windows() { windows = latest; }
        for resource in &pending {
            if matched.contains_key(&resource.id) { continue; }
            if let Some(window) = windows.iter().find(|window| !used.contains(&window.address) && resource_matches(resource, window)).cloned() {
                used.insert(window.address.clone());
                matched.insert(resource.id.clone(), (window, false));
            }
        }
    }

    let mut items = Vec::new();
    for resource in &stage.resources {
        if resource.kind == "note" {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "skipped".into(), message: "Resumption note is available in Stage".into() });
        } else if let Some(message) = failures.get(&resource.id) {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: message.clone() });
        } else if let Some((window, reused)) = matched.get(&resource.id) {
            match place_window(resource, window) {
                Ok(()) => items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: if *reused { "reused".into() } else { "placed".into() }, message: format!("{} on workspace {}", if *reused { "Reused existing window" } else { "Launched and placed" }, resource.placement.workspace) }),
                Err(error) => items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: format!("Window found, but placement failed: {error}") }),
            }
        } else {
            items.push(RestoreItem { resource_id: resource.id.clone(), name: resource.name.clone(), status: "failed".into(), message: "No matching window appeared within 10 seconds".into() });
        }
    }
    let _ = hypr_dispatch("workspace", &stage.focus_workspace.to_string());
    let failed = items.iter().filter(|item| item.status == "failed").count();
    let handled = items.iter().filter(|item| item.status != "skipped").count();
    let status = if failed == 0 { "success" } else if failed < handled { "partial" } else { "failed" };
    let run = RestoreRun { id: Uuid::new_v4().to_string(), stage_id: stage.id, stage_name: stage.name, started_at, duration_ms: started.elapsed().as_millis(), status: status.into(), items };
    let document = serde_json::to_string(&run).map_err(|error| error.to_string())?;
    let connection = Connection::open(&state.db_path).map_err(|error| error.to_string())?;
    connection.execute("INSERT INTO restore_runs (id,started_at,document) VALUES (?1,?2,?3)", params![run.id, run.started_at, document]).map_err(|error| error.to_string())?;
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
            app.manage(AppState { db_path });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![list_stages, save_stage, delete_stage, discover_apps, system_snapshot, capture_current, restore_stage, list_runs])
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
    fn requires_a_class_rule_before_matching() {
        let resource = StageResource { id: "r".into(), kind: "app".into(), name: "Browser".into(), target: String::new(), command: vec!["firefox".into()], working_directory: String::new(), match_class: "firefox".into(), match_title: "Docs".into(), desktop_file: None, icon: None, placement: Placement { workspace: 1, monitor: String::new(), mode: "tiled".into(), pinned: false, x: None, y: None, width: None, height: None }, confirmed: true };
        let window = HyprWindow { address: "0x1".into(), class_name: "firefox".into(), title: "Rust Docs".into(), workspace: 1, monitor: "DP-1".into(), pid: 1, floating: false, fullscreen: false, pinned: false };
        assert!(resource_matches(&resource, &window));
        assert!(!resource_matches(&StageResource { match_class: String::new(), ..resource }, &window));
    }
}
