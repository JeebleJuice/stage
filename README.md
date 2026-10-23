# Stage

Stage is a Hyprland-first desktop application that saves the resources and layout needed for an activity, then reconstructs that working environment after applications close or the computer restarts.

The project follows the product definition in `Stage - Persistent Hyprland Workspaces.md` from the Obsidian ideas folder.

## What works

- Create, edit, duplicate, delete, import, and export Stage definitions.
- Discover installed Linux applications from standard `.desktop` entries.
- Add applications, files, folders, URLs, terminal directories, reviewed commands, and resumption notes.
- Capture the current Hyprland window arrangement into a safe draft, resolve installed-application identities where possible, and avoid silently collecting commands or URLs.
- Persist Stage definitions and restoration history in local SQLite.
- Reuse matching open windows before launching missing resources.
- Place windows through Hyprland dispatchers, including workspace, monitor, tiled/floating/fullscreen, pinned state, and optional floating geometry.
- Stream per-resource restoration progress, support cancellation, and report each reused, launched, placed, skipped, timed-out, or failed resource.
- Run the UI in a browser with a safe simulated adapter for interface development.

Stage restores declared resources and placement. It does not claim to reconstruct arbitrary unsaved state inside another application.

## Run the interface

```bash
npm install
npm run dev
```

The browser preview uses simulated desktop data and never launches local applications.

## Run the desktop app on Fedora + Hyprland

Install Tauri's Linux prerequisites once:

```bash
sudo dnf install webkit2gtk4.1-devel libsoup3-devel
```

Then run:

```bash
npm run desktop
```

Stage uses `hyprctl -j` for state and supported Hyprland dispatchers for placement. If Kitty is not your terminal, set `STAGE_TERMINAL` or `TERMINAL` to a compatible terminal command. Terminal commands are passed as explicit argument arrays and are blocked until reviewed in the editor.

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
