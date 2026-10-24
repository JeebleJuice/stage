import type { StageDefinition } from "./types";

const now = new Date().toISOString();

export const sampleStages: StageDefinition[] = [{
  id: "fenjalbum-development",
  name: "Fenjalbum development",
  description: "Code, application, files and project notes",
  color: "#d7ff72",
  notes: "Pick up from the current gallery and worker tasks.",
  clearWorkspaces: true,
  focusWorkspace: 1,
  createdAt: now,
  updatedAt: now,
  resources: [
    { id: "editor", kind: "app", name: "Editor", target: "/home/jeeblejuice/Dev/Fenjalbum", command: ["code"], workingDirectory: "/home/jeeblejuice/Dev/Fenjalbum", matchClass: "code", matchTitle: "Fenjalbum", placement: { workspace: 1, monitor: "", mode: "tiled", pinned: false }, confirmed: true },
    { id: "dev-server", kind: "terminal", name: "Development server", target: "npm run dev", command: ["npm", "run", "dev"], workingDirectory: "/home/jeeblejuice/Dev/Fenjalbum", matchClass: "kitty", matchTitle: "", placement: { workspace: 1, monitor: "", mode: "tiled", pinned: false }, confirmed: false },
    { id: "application", kind: "url", name: "Application", target: "http://localhost:3000", command: [], workingDirectory: "", matchClass: "firefox", matchTitle: "", placement: { workspace: 2, monitor: "", mode: "tiled", pinned: false }, confirmed: true },
    { id: "screenshots", kind: "folder", name: "Screenshots", target: "/home/jeeblejuice/Dev/Fenjalbum/screenshots", command: [], workingDirectory: "", matchClass: "org.kde.dolphin", matchTitle: "screenshots", placement: { workspace: 3, monitor: "", mode: "tiled", pinned: false }, confirmed: true },
    { id: "resume-note", kind: "note", name: "Resumption note", target: "Review the current task list before coding.", command: [], workingDirectory: "", matchClass: "", matchTitle: "", placement: { workspace: 3, monitor: "", mode: "tiled", pinned: false }, confirmed: true }
  ]
}];
