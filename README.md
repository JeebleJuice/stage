# Stage

Stage is a Hyprland-first desktop application that saves the resources and layout needed for an activity, then reconstructs that working environment after applications close or the computer restarts.

The project follows the product definition in `Stage - Persistent Hyprland Workspaces.md` from the Obsidian ideas folder.

## What works

- Create, edit, duplicate, delete, import, and export Stage definitions.
- Discover installed Linux applications from standard `.desktop` entries.
- Discover `~/Dev` projects with an `npm run dev` script and create a complete development Stage for each one.
- Add applications, files, folders, URLs, terminal directories, reviewed commands, and resumption notes.
- Capture the current Hyprland window arrangement into a safe draft, resolve installed-application identities where possible, and avoid silently collecting commands or URLs.
- Persist Stage definitions and restoration history in local SQLite.
- Reuse matching open windows before launching missing resources.
- Place windows through Hyprland dispatchers, including workspace, monitor, tiled/floating/fullscreen, pinned state, and optional floating geometry.
- Close every application window on a Stage's declared workspaces in one confirmed action, while keeping Stage itself open.
- Stream per-resource restoration progress, support cancellation, and report each reused, launched, placed, skipped, timed-out, or failed resource.
- Run the UI in a browser with a safe simulated adapter for interface development.

Stage restores declared resources and placement. It does not claim to reconstruct arbitrary unsaved state inside another application.

## Run the interface

```bash
npm install
npm run dev
```

The browser preview uses simulated desktop data and never launches local applications.

On its first native launch, Stage discovers npm development projects under `~/Dev` and adds any missing generated definitions. Before restoring a generated development Stage, it moves every existing window on workspaces 1–3 into the first empty three-workspace block at workspace 4 or later. It then starts the dev server and a Codex session on workspace 1, waits for the detected local HTTP endpoint before starting `npm run worker` when that script exists, opens the ready application in a new default-browser window on workspace 2, and opens the project in VS Code on workspace 3. Project database prerequisites declared by the generated Stage run before the dev server, and held terminals with an exited command are relaunched instead of reused. Use **Settings → Sync projects** after adding another project. Synchronisation refreshes generated launch, prerequisite, readiness, and make-room details while preserving personal placement edits.

## Run the desktop app on Fedora + Hyprland

Install Tauri's Linux prerequisites once:

```bash
sudo dnf install webkit2gtk4.1-devel libsoup3-devel
```

Then run:

```bash
npm run desktop
```

Stage uses `hyprctl -j` for state and Hyprland 0.56's Lua dispatcher API for placement. Windows are moved as soon as they are matched instead of waiting for the whole restoration attempt to finish. If Kitty is not your terminal, set `STAGE_TERMINAL` or `TERMINAL` to a compatible terminal command. Terminal commands are passed as explicit argument arrays and are blocked until reviewed in the editor.

## Global shortcut

After installing the packaged app, add this to `~/.config/hypr/hyprland.conf`:

```ini
bind = SUPER, S, exec, stage
```

Reload Hyprland with `hyprctl reload`.

## Checks and packaging

```bash
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
npm run desktop:build
```

The bundle configuration targets RPM and AppImage. Local application data is stored in the platform app-data directory as `stage.sqlite3`.

## Matching model

Stage scores existing and newly launched windows using configured class, optional title-substring rules, executable identity, and process ancestry. Captured Stages begin with the Hyprland initial window class and automatically attach a matching desktop entry when one can be resolved. Review unresolved resources and select their installed application or provide an explicit launch target before relying on them after a reboot.
