import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { InstalledApp, RestoreProgress, RestoreRun, StageDefinition, SystemSnapshot } from "./types";

const STORAGE_KEY = "stage-definitions-v1";
const RUN_KEY = "stage-runs-v1";
const isTauri = () => "__TAURI_INTERNALS__" in window;

const readLocal = <T>(key: string, fallback: T): T => {
  try { return JSON.parse(localStorage.getItem(key) ?? "") as T; } catch { return fallback; }
};

export async function listStages(fallback: StageDefinition[]): Promise<StageDefinition[]> {
  if (isTauri()) return invoke<StageDefinition[]>("list_stages");
  return readLocal(STORAGE_KEY, fallback);
}

export async function saveStage(stage: StageDefinition): Promise<void> {
  if (isTauri()) return invoke("save_stage", { stage });
  const stages = readLocal<StageDefinition[]>(STORAGE_KEY, []);
  localStorage.setItem(STORAGE_KEY, JSON.stringify([...stages.filter((item) => item.id !== stage.id), stage]));
}

export async function deleteStage(id: string): Promise<void> {
  if (isTauri()) return invoke("delete_stage", { id });
  localStorage.setItem(STORAGE_KEY, JSON.stringify(readLocal<StageDefinition[]>(STORAGE_KEY, []).filter((item) => item.id !== id)));
}

export async function getSnapshot(): Promise<SystemSnapshot> {
  if (isTauri()) return invoke("system_snapshot");
  return { connected: false, hyprlandVersion: "Browser preview", monitors: ["DP-1", "HDMI-A-1"], workspaces: [1, 2, 3], windows: [], message: "Open the Tauri app to connect to Hyprland." };
}

export async function captureCurrent(): Promise<StageDefinition> {
  if (isTauri()) return invoke("capture_current");
  throw new Error("Capture requires the Stage desktop app.");
}

export async function discoverApps(): Promise<InstalledApp[]> {
  if (isTauri()) return invoke("discover_apps");
  return [
    { name: "Firefox", description: "Web Browser", exec: ["firefox"], icon: "firefox", desktopFile: "firefox.desktop", wmClass: "firefox" },
    { name: "Kitty", description: "Terminal emulator", exec: ["kitty"], icon: "kitty", desktopFile: "kitty.desktop", wmClass: "kitty" },
    { name: "Dolphin", description: "File manager", exec: ["dolphin"], icon: "system-file-manager", desktopFile: "org.kde.dolphin.desktop", wmClass: "org.kde.dolphin" },
    { name: "Obsidian", description: "Notes", exec: ["obsidian"], icon: "obsidian", desktopFile: "obsidian.desktop", wmClass: "obsidian" }
  ];
}

export async function discoverDevelopmentStages(root?: string): Promise<StageDefinition[]> {
  if (isTauri()) return invoke("discover_development_stages", { root });
  return [];
}

export async function restoreStage(stage: StageDefinition, operationId: string): Promise<RestoreRun> {
  if (isTauri()) return invoke("restore_stage", { stage, operationId });
  const items = stage.resources.map((resource) => ({ resourceId: resource.id, name: resource.name, status: resource.kind === "note" ? "skipped" as const : "placed" as const, message: resource.kind === "note" ? "Note shown in Stage" : `Previewed on workspace ${resource.placement.workspace}` }));
  const run: RestoreRun = { id: operationId, stageId: stage.id, stageName: stage.name, startedAt: new Date().toISOString(), durationMs: 640, status: "success", items };
  localStorage.setItem(RUN_KEY, JSON.stringify([run, ...readLocal<RestoreRun[]>(RUN_KEY, [])].slice(0, 50)));
  await new Promise((resolve) => setTimeout(resolve, 900));
  return run;
}

export async function cancelRestore(operationId: string): Promise<void> {
  if (isTauri()) return invoke("cancel_restore", { operationId });
}

export async function closeStage(stage: StageDefinition): Promise<number> {
  if (isTauri()) return invoke<number>("close_stage", { stage });
  return 0;
}

export async function onRestoreProgress(handler: (progress: RestoreProgress) => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => undefined;
  return listen<RestoreProgress>("stage://restore-progress", (event) => handler(event.payload));
}

export async function listRuns(): Promise<RestoreRun[]> {
  if (isTauri()) return invoke("list_runs");
  return readLocal(RUN_KEY, []);
}
