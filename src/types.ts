export type ResourceKind = "app" | "file" | "folder" | "url" | "terminal" | "note";
export type WindowMode = "tiled" | "floating" | "fullscreen";

export interface Placement {
  workspace: number;
  monitor: string;
  mode: WindowMode;
  pinned: boolean;
  x?: number;
  y?: number;
  width?: number;
  height?: number;
}

export interface StageResource {
  id: string;
  kind: ResourceKind;
  name: string;
  target: string;
  command: string[];
  workingDirectory: string;
  matchClass: string;
  matchTitle: string;
  desktopFile?: string;
  icon?: string;
  waitForUrl?: string;
  placement: Placement;
  confirmed: boolean;
}

export interface StageDefinition {
  id: string;
  name: string;
  description: string;
  color: string;
  notes: string;
  focusWorkspace: number;
  createdAt: string;
  updatedAt: string;
  resources: StageResource[];
}

export interface InstalledApp {
  name: string;
  description: string;
  exec: string[];
  icon: string;
  desktopFile: string;
  wmClass: string;
}

export interface HyprWindow {
  address: string;
  className: string;
  title: string;
  workspace: number;
  monitor: string;
  pid: number;
  floating: boolean;
  fullscreen: boolean;
  pinned: boolean;
}

export interface SystemSnapshot {
  connected: boolean;
  hyprlandVersion: string;
  monitors: string[];
  workspaces: number[];
  windows: HyprWindow[];
  message: string;
}

export interface RestoreItem {
  resourceId: string;
  name: string;
  status: "reused" | "launched" | "placed" | "failed" | "skipped";
  message: string;
}

export interface RestoreRun {
  id: string;
  stageId: string;
  stageName: string;
  startedAt: string;
  durationMs: number;
  status: "success" | "partial" | "failed" | "cancelled";
  items: RestoreItem[];
}

export interface RestoreProgress {
  operationId: string;
  phase: "matching" | "launching" | "waiting" | "placing" | "complete" | "cancelled";
  item?: RestoreItem;
  message: string;
}
