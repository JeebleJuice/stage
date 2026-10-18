import { useEffect, useMemo, useRef, useState } from "react";
import { AppWindow, Archive, ArrowLeft, Check, ChevronRight, CircleAlert, CircleDot, Copy, Download, ExternalLink, File, Folder, History, LayoutDashboard, Link2, ListRestart, Monitor, MoreHorizontal, PanelLeft, Pencil, Pin, Plus, Search, Settings, Sparkles, SquareTerminal, Trash2, Upload, Waypoints, X } from "lucide-react";
import { captureCurrent, deleteStage, discoverApps, getSnapshot, listRuns, listStages, restoreStage, saveStage } from "./bridge";
import { sampleStages } from "./data";
import type { InstalledApp, ResourceKind, RestoreRun, StageDefinition, StageResource, SystemSnapshot, WindowMode } from "./types";

const colors = ["#d7ff72", "#82d8ff", "#ffab76", "#c7a5ff", "#ff83ad"];
const kindIcon = { app: AppWindow, file: File, folder: Folder, url: Link2, terminal: SquareTerminal, note: Pencil };
const emptyPlacement = () => ({ workspace: 1, monitor: "", mode: "tiled" as WindowMode, pinned: false });

function newResource(kind: ResourceKind): StageResource {
  return { id: crypto.randomUUID(), kind, name: kind === "url" ? "Website" : kind === "terminal" ? "Terminal" : kind[0].toUpperCase() + kind.slice(1), target: "", command: [], workingDirectory: "", matchClass: "", matchTitle: "", placement: emptyPlacement(), confirmed: kind !== "terminal", };
}

function resourceSubtitle(resource: StageResource) {
  if (resource.kind === "note") return resource.target || "No note yet";
  if (resource.kind === "terminal") return `${resource.workingDirectory || "Home"} · ${resource.command.join(" ") || "Shell"}`;
  return resource.target || resource.command.join(" ") || "Not configured";
}

function LayoutPreview({ stage, large = false }: { stage: StageDefinition; large?: boolean }) {
  const workspaces = [...new Set(stage.resources.filter((item) => item.kind !== "note").map((item) => item.placement.workspace))].sort((a, b) => a - b);
  return <div className={`layout-preview ${large ? "large" : ""}`}>
    {(workspaces.length ? workspaces : [1]).slice(0, large ? 6 : 3).map((workspace) => <div className="workspace-mini" key={workspace}>
      <span>{workspace}</span>
      <div className="window-stack">{stage.resources.filter((item) => item.kind !== "note" && item.placement.workspace === workspace).slice(0, 4).map((item) => <i key={item.id} className={item.placement.mode} title={item.name} />)}</div>
    </div>)}
  </div>;
}

function StageCard({ stage, onOpen, onRestore }: { stage: StageDefinition; onOpen: () => void; onRestore: () => void }) {
  return <article className="stage-card" style={{ "--accent": stage.color } as React.CSSProperties}>
    <button className="card-main" onClick={onOpen}>
      <div className="card-top"><span className="stage-orbit"><i /><i /></span><span className="status-tag">Ready</span><MoreHorizontal size={17} /></div>
      <h3>{stage.name}</h3><p>{stage.description}</p><LayoutPreview stage={stage} />
      <div className="card-meta"><span>{stage.resources.length} resources</span><span>{new Set(stage.resources.map((item) => item.placement.workspace)).size} workspaces</span></div>
    </button>
    <button className="restore-card" onClick={onRestore}><ListRestart size={15} /> Restore stage <ChevronRight size={15} /></button>
  </article>;
}

function ResourceEditor({ resource, monitors, onChange, onRemove }: { resource: StageResource; monitors: string[]; onChange: (resource: StageResource) => void; onRemove: () => void }) {
  const Icon = kindIcon[resource.kind];
  const update = <K extends keyof StageResource>(key: K, value: StageResource[K]) => onChange({ ...resource, [key]: value });
  const place = <K extends keyof StageResource["placement"]>(key: K, value: StageResource["placement"][K]) => onChange({ ...resource, placement: { ...resource.placement, [key]: value } });
  return <div className="resource-editor">
    <div className="resource-editor-title"><span className={`resource-icon ${resource.kind}`}><Icon size={17} /></span><div><input value={resource.name} onChange={(event) => update("name", event.target.value)} /><small>{resource.kind}</small></div><button className="icon-button danger" onClick={onRemove} aria-label="Remove resource"><Trash2 size={15} /></button></div>
    {resource.kind === "note" ? <label className="wide"><span>Note</span><textarea value={resource.target} onChange={(event) => update("target", event.target.value)} placeholder="What should future-you know?" /></label> : <>
      <div className="field-grid">
        <label><span>{resource.kind === "url" ? "URL" : resource.kind === "terminal" ? "Command arguments" : "Path or target"}</span><input value={resource.kind === "terminal" ? resource.command.join(" ") : resource.target} onChange={(event) => resource.kind === "terminal" ? update("command", event.target.value.split(" ").filter(Boolean)) : update("target", event.target.value)} placeholder={resource.kind === "url" ? "https://…" : "/home/…"} /></label>
        <label><span>Working directory</span><input value={resource.workingDirectory} onChange={(event) => update("workingDirectory", event.target.value)} placeholder="Optional" /></label>
        <label><span>Match window class</span><input value={resource.matchClass} onChange={(event) => update("matchClass", event.target.value)} placeholder="e.g. firefox" /></label>
        <label><span>Match title</span><input value={resource.matchTitle} onChange={(event) => update("matchTitle", event.target.value)} placeholder="Optional contains rule" /></label>
      </div>
      <div className="placement-row">
        <label><span>Workspace</span><input type="number" min="1" value={resource.placement.workspace} onChange={(event) => place("workspace", Number(event.target.value) || 1)} /></label>
        <label><span>Monitor</span><select value={resource.placement.monitor} onChange={(event) => place("monitor", event.target.value)}><option value="">Current / automatic</option>{monitors.map((monitor) => <option key={monitor}>{monitor}</option>)}</select></label>
        <label><span>Window mode</span><select value={resource.placement.mode} onChange={(event) => place("mode", event.target.value as WindowMode)}><option value="tiled">Tiled</option><option value="floating">Floating</option><option value="fullscreen">Fullscreen</option></select></label>
        <button className={`pin-button ${resource.placement.pinned ? "active" : ""}`} onClick={() => place("pinned", !resource.placement.pinned)}><Pin size={14} /> Pinned</button>
      </div>
      {resource.kind === "terminal" && resource.command.length > 0 && <label className="confirmation"><input type="checkbox" checked={resource.confirmed} onChange={(event) => update("confirmed", event.target.checked)} /><span>I reviewed this command and allow Stage to run it.</span></label>}
    </>}
  </div>;
}

function AddResourceMenu({ apps, onAdd, onClose }: { apps: InstalledApp[]; onAdd: (resource: StageResource) => void; onClose: () => void }) {
  const [query, setQuery] = useState("");
  const filtered = apps.filter((app) => `${app.name} ${app.description}`.toLowerCase().includes(query.toLowerCase())).slice(0, 10);
  const basic: { kind: ResourceKind; label: string; text: string }[] = [
    { kind: "terminal", label: "Terminal", text: "Directory or reviewed command" }, { kind: "folder", label: "Folder", text: "Open in the default file manager" }, { kind: "file", label: "File", text: "Open with its default application" }, { kind: "url", label: "URL", text: "Open in the default browser" }, { kind: "note", label: "Resumption note", text: "Context without launching a window" }
  ];
  return <div className="popover add-menu"><div className="popover-head"><strong>Add a resource</strong><button onClick={onClose}><X size={16} /></button></div><label className="app-search"><Search size={15} /><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Find an installed application" /></label>
    {filtered.length > 0 && <div className="menu-section"><small>INSTALLED APPLICATIONS</small>{filtered.map((app) => <button key={app.desktopFile} onClick={() => onAdd({ ...newResource("app"), name: app.name, command: app.exec, matchClass: app.wmClass, desktopFile: app.desktopFile, icon: app.icon })}><span className="app-avatar">{app.name.slice(0, 1)}</span><span><b>{app.name}</b><em>{app.description || app.exec.join(" ")}</em></span><Plus size={15} /></button>)}</div>}
    <div className="menu-section"><small>OTHER RESOURCES</small>{basic.map(({ kind, label, text }) => { const Icon = kindIcon[kind]; return <button key={kind} onClick={() => onAdd(newResource(kind))}><span className={`resource-icon ${kind}`}><Icon size={16} /></span><span><b>{label}</b><em>{text}</em></span><Plus size={15} /></button>; })}</div>
  </div>;
}

function RestoreDialog({ stage, run, running, onClose }: { stage: StageDefinition; run?: RestoreRun; running: boolean; onClose: () => void }) {
  return <div className="modal-backdrop"><section className="restore-dialog">
    <div className="restore-visual"><span className={running ? "pulse-ring running" : "pulse-ring"}><Waypoints size={30} /></span><div><span className="eyebrow">{running ? "Restoring your workspace" : run?.status === "success" ? "Stage is ready" : "Restoration finished"}</span><h2>{stage.name}</h2><p>{running ? "Matching open windows and launching what is missing…" : `${run?.items.filter((item) => item.status !== "failed").length ?? 0} of ${run?.items.length ?? 0} resources handled`}</p></div>{!running && <button onClick={onClose}><X size={18} /></button>}</div>
    <div className="restore-items">{stage.resources.map((resource, index) => { const Icon = kindIcon[resource.kind]; const item = run?.items.find((result) => result.resourceId === resource.id); return <div key={resource.id} className={item?.status === "failed" ? "failed" : ""}><span className={`resource-icon ${resource.kind}`}><Icon size={16} /></span><span><b>{resource.name}</b><small>{item?.message || (running ? index === 0 ? "Checking…" : "Waiting" : "Not reported")}</small></span><i>{running ? <span className="loader" /> : item?.status === "failed" ? <CircleAlert size={17} /> : <Check size={17} />}</i></div>; })}</div>
    {!running && <button className="primary-wide" onClick={onClose}>Return to Stage</button>}
  </section></div>;
}

export default function App() {
  const [stages, setStages] = useState<StageDefinition[]>(sampleStages);
  const [activeId, setActiveId] = useState<string>();
  const [view, setView] = useState<"stages" | "history" | "settings">("stages");
  const [snapshot, setSnapshot] = useState<SystemSnapshot>({ connected: false, hyprlandVersion: "Checking…", monitors: [], workspaces: [], windows: [], message: "" });
  const [apps, setApps] = useState<InstalledApp[]>([]);
  const [runs, setRuns] = useState<RestoreRun[]>([]);
  const [addOpen, setAddOpen] = useState(false);
  const [restoring, setRestoring] = useState<StageDefinition>();
  const [restoreRun, setRestoreRun] = useState<RestoreRun>();
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState("");
  const importInput = useRef<HTMLInputElement>(null);
  const active = stages.find((stage) => stage.id === activeId);

  useEffect(() => { Promise.all([listStages(sampleStages), getSnapshot(), discoverApps(), listRuns()]).then(([saved, system, foundApps, history]) => { setStages(saved.length ? saved : sampleStages); setSnapshot(system); setApps(foundApps); setRuns(history); }).catch((error) => setNotice(String(error))); }, []);

  const persist = (stage: StageDefinition) => { const next = { ...stage, updatedAt: new Date().toISOString() }; setStages((all) => all.map((item) => item.id === next.id ? next : item)); void saveStage(next).catch((error) => setNotice(String(error))); };
  const createStage = () => { const now = new Date().toISOString(); const stage: StageDefinition = { id: crypto.randomUUID(), name: "Untitled stage", description: "A new persistent workspace", color: colors[stages.length % colors.length], notes: "", focusWorkspace: 1, createdAt: now, updatedAt: now, resources: [] }; setStages((all) => [...all, stage]); setActiveId(stage.id); void saveStage(stage); };
  const duplicateActive = () => { if (!active) return; const copy = { ...active, id: crypto.randomUUID(), name: `${active.name} copy`, createdAt: new Date().toISOString(), updatedAt: new Date().toISOString(), resources: active.resources.map((item) => ({ ...item, id: crypto.randomUUID() })) }; setStages((all) => [...all, copy]); setActiveId(copy.id); void saveStage(copy); };
  const removeActive = async () => { if (!active || !confirm(`Delete “${active.name}”?`)) return; await deleteStage(active.id); setStages((all) => all.filter((item) => item.id !== active.id)); setActiveId(undefined); };
  const runRestore = async (stage: StageDefinition) => { setRestoring(stage); setRestoreRun(undefined); try { const result = await restoreStage(stage); setRestoreRun(result); setRuns((all) => [result, ...all.filter((item) => item.id !== result.id)]); } catch (error) { setRestoring(undefined); setNotice(String(error)); } };
  const capture = async () => { try { const draft = await captureCurrent(); setStages((all) => [...all, draft]); setActiveId(draft.id); void saveStage(draft); } catch (error) { setNotice(String(error)); } };
  const exportStage = () => { if (!active) return; const href = URL.createObjectURL(new Blob([JSON.stringify(active, null, 2)], { type: "application/json" })); const anchor = document.createElement("a"); anchor.href = href; anchor.download = `${active.name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}.stage.json`; anchor.click(); URL.revokeObjectURL(href); };
  const importStage = async (file?: File) => { if (!file) return; try { const value = JSON.parse(await file.text()) as StageDefinition; if (!value.name || !Array.isArray(value.resources)) throw new Error("Invalid Stage file"); value.id = crypto.randomUUID(); value.updatedAt = new Date().toISOString(); setStages((all) => [...all, value]); setActiveId(value.id); await saveStage(value); } catch (error) { setNotice(String(error)); } };
  const filtered = useMemo(() => stages.filter((stage) => `${stage.name} ${stage.description}`.toLowerCase().includes(query.toLowerCase())), [stages, query]);

  return <div className="app-shell">
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark"><i /><i /><i /></span><div><strong>STAGE</strong><small>HYPRLAND SESSIONS</small></div></div>
      <nav><button className={view === "stages" ? "active" : ""} onClick={() => { setView("stages"); setActiveId(undefined); }}><LayoutDashboard size={17} /> Stages <span>{stages.length}</span></button><button className={view === "history" ? "active" : ""} onClick={() => { setView("history"); setActiveId(undefined); }}><History size={17} /> History</button></nav>
      <div className="sidebar-label">YOUR STAGES</div><div className="stage-links">{stages.map((stage) => <button key={stage.id} className={activeId === stage.id ? "active" : ""} onClick={() => { setActiveId(stage.id); setView("stages"); }}><i style={{ background: stage.color }} /><span>{stage.name}</span><small>{stage.resources.length}</small></button>)}</div>
      <div className="sidebar-bottom"><button onClick={() => setView("settings")} className={view === "settings" ? "active" : ""}><Settings size={17} /> Settings</button><div className={`hypr-status ${snapshot.connected ? "online" : ""}`}><CircleDot size={15} /><span><b>{snapshot.connected ? "Hyprland connected" : "Desktop preview"}</b><small>{snapshot.monitors.length} monitors · {snapshot.windows.length} windows</small></span></div></div>
    </aside>

    <main className="main-view">
      {!active && view === "stages" && <>
        <header className="topbar"><div className="search"><Search size={16} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Find a stage…" /><kbd>⌘ K</kbd></div><div className="top-actions"><button onClick={() => importInput.current?.click()}><Upload size={15} /> Import</button><input ref={importInput} type="file" accept="application/json,.json" hidden onChange={(event) => importStage(event.target.files?.[0])} /><button className="capture-button" onClick={capture}><Sparkles size={15} /> Capture current setup</button></div></header>
        <div className="page-content"><div className="hero-row"><div><span className="eyebrow">PERSISTENT WORKSPACES</span><h1>Your work, <em>back in place.</em></h1><p>Reopen the tools, files and context for an activity — arranged across Hyprland exactly where they belong.</p></div><button className="new-stage" onClick={createStage}><Plus size={20} /><span><b>Create a stage</b><small>Build a workspace from scratch</small></span></button></div>
          <div className="section-title"><div><h2>Stages</h2><span>{filtered.length} saved environments</span></div><button><PanelLeft size={15} /> Grid</button></div>
          <div className="stage-grid">{filtered.map((stage) => <StageCard key={stage.id} stage={stage} onOpen={() => setActiveId(stage.id)} onRestore={() => runRestore(stage)} />)}{!filtered.length && <div className="empty-state"><Waypoints size={38} /><h3>No matching stages</h3><p>Try a different search or create a fresh workspace.</p></div>}</div>
          <div className="honesty-note"><CircleAlert size={17} /><span><b>Stage restores declared resources and window placement.</b> It does not claim to recover unsaved application state.</span></div>
        </div>
      </>}

      {active && view === "stages" && <>
        <header className="editor-topbar"><button className="back" onClick={() => setActiveId(undefined)}><ArrowLeft size={17} /> All stages</button><div className="saved-state"><Check size={14} /> Saved locally</div><button onClick={duplicateActive}><Copy size={15} /> Duplicate</button><button onClick={exportStage}><Download size={15} /> Export</button><button className="icon-button danger" onClick={removeActive}><Trash2 size={16} /></button><button className="primary" onClick={() => runRestore(active)}><ListRestart size={16} /> Restore stage</button></header>
        <div className="editor-page"><section className="stage-summary" style={{ "--accent": active.color } as React.CSSProperties}><div className="stage-heading"><span className="stage-orbit big"><i /><i /></span><div><input className="stage-name" value={active.name} onChange={(event) => persist({ ...active, name: event.target.value })} /><input className="stage-description" value={active.description} onChange={(event) => persist({ ...active, description: event.target.value })} /></div><div className="color-picker">{colors.map((color) => <button key={color} className={active.color === color ? "active" : ""} style={{ background: color }} onClick={() => persist({ ...active, color })} />)}</div></div><LayoutPreview stage={active} large /><div className="summary-stats"><span><Monitor size={15} /><b>{new Set(active.resources.map((item) => item.placement.workspace)).size || 0}</b> workspaces</span><span><AppWindow size={15} /><b>{active.resources.filter((item) => item.kind !== "note").length}</b> windows</span><label>Finish on workspace <input type="number" min="1" value={active.focusWorkspace} onChange={(event) => persist({ ...active, focusWorkspace: Number(event.target.value) || 1 })} /></label></div></section>
          <section className="resource-section"><div className="resource-heading"><div><span className="eyebrow">STAGE CONTENTS</span><h2>Resources & placement</h2><p>Stage reuses matching windows before it launches anything new.</p></div><div className="add-wrap"><button className="add-resource" onClick={() => setAddOpen(!addOpen)}><Plus size={16} /> Add resource</button>{addOpen && <AddResourceMenu apps={apps} onClose={() => setAddOpen(false)} onAdd={(resource) => { persist({ ...active, resources: [...active.resources, resource] }); setAddOpen(false); }} />}</div></div>
            <div className="resource-list">{active.resources.map((resource, index) => <div className="resource-row" key={resource.id}><span className="drag-index">{String(index + 1).padStart(2, "0")}</span><div className="resource-compact"><span className={`resource-icon ${resource.kind}`}>{(() => { const Icon = kindIcon[resource.kind]; return <Icon size={17} />; })()}</span><div><b>{resource.name}</b><small>{resourceSubtitle(resource)}</small></div><span className="workspace-pill">WS {resource.placement.workspace}</span></div><ResourceEditor resource={resource} monitors={snapshot.monitors} onChange={(next) => persist({ ...active, resources: active.resources.map((item) => item.id === next.id ? next : item) })} onRemove={() => persist({ ...active, resources: active.resources.filter((item) => item.id !== resource.id) })} /></div>)}{!active.resources.length && <div className="empty-resources"><Archive size={34} /><h3>This stage is empty</h3><p>Add an installed app, path, URL, terminal or resumption note.</p></div>}</div>
          </section>
        </div>
      </>}

      {view === "history" && <div className="simple-page"><span className="eyebrow">RESTORATION LOG</span><h1>History</h1><p>Every attempt stays visible, including resources Stage could not place.</p><div className="run-list">{runs.map((run) => <article key={run.id}><span className={`run-icon ${run.status}`}><History size={18} /></span><div><b>{run.stageName}</b><small>{new Date(run.startedAt).toLocaleString()} · {run.durationMs} ms</small></div><span>{run.items.filter((item) => item.status !== "failed").length}/{run.items.length} resources</span><i className={run.status}>{run.status}</i></article>)}{!runs.length && <div className="empty-resources"><History size={34} /><h3>No restorations yet</h3><p>Restore a stage and its result will appear here.</p></div>}</div></div>}

      {view === "settings" && <div className="simple-page settings-page"><span className="eyebrow">SYSTEM</span><h1>Stage settings</h1><p>Stage talks to Hyprland through its supported <code>hyprctl -j</code> interface and dispatchers.</p><div className="settings-card"><div><CircleDot size={17} /><span><b>Hyprland connection</b><small>{snapshot.message}</small></span><strong className={snapshot.connected ? "ok" : ""}>{snapshot.connected ? "Connected" : "Unavailable"}</strong></div><div><Monitor size={17} /><span><b>Detected monitors</b><small>{snapshot.monitors.join(", ") || "No monitors reported"}</small></span><strong>{snapshot.monitors.length}</strong></div><div><AppWindow size={17} /><span><b>Open windows</b><small>Available for capture and reuse</small></span><strong>{snapshot.windows.length}</strong></div><div><ExternalLink size={17} /><span><b>Global shortcut</b><small>Add <code>bind = SUPER, S, exec, stage</code> to hyprland.conf after installing.</small></span></div></div></div>}
    </main>
    {restoring && <RestoreDialog stage={restoring} run={restoreRun} running={!restoreRun} onClose={() => { setRestoring(undefined); setRestoreRun(undefined); }} />}
    {notice && <button className="toast" onClick={() => setNotice("")}><CircleAlert size={17} /><span>{notice}</span><X size={14} /></button>}
  </div>;
}
