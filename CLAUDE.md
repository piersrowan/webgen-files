# CLAUDE.md — webgen-files

You own **`webgen-files`**, one app in the WebGen family. The shared
contract/design/coordination lives in `../webgen-distro` — read it first (`CONTRACT.md`,
`DESIGN-SYSTEM.md`, `INTEGRATION.md`).

## What this is
A GTK4 / libadwaita **file manager**: LHS collapsible folder tree, RHS files + subfolders with
icons, Back/Forward/Up + address bar, New Folder, Copy/Paste, Open, Open With…, and a
filter/search box scoped to the current folder (with a Recursive option).

## Stack (match the family)
Rust, GTK4 0.9 / libadwaita 0.7 (`v1_4` — do **not** bump to `v1_5`/AlertDialog unless the OS
libadwaita is confirmed ≥ 1.5). No relm4 — the list/tree views are driven directly with GTK list
models, which relm4's component model doesn't fit.

## Structure
```
src/main.rs   # adw app: window, navigation, file ListView, actions, dialogs
src/entry.rs  # Entry row type + directory listing + recursive search + recursive copy
src/tree.rs   # LHS folder tree (TreeListModel + TreeExpander)
src/assoc.rs  # the shared "Opens With" association store (registry namespace com.webgen.OpensWith)
packaging/    # com.webgen.Files.desktop, settings manifest, icons (teal folder), deb-scripts
```

## Identity contract
appId / GApplication id / `.desktop StartupWMClass` all = **`com.webgen.Files`**; binary
`webgen-files`; Categories `System;Utility;FileManager;`. Icon = family concentric frame + teal
`#0e9488` **folder** (regenerate from `../webgen-distro/icons/generate_icons.py`).

## The "Opens With" association store — SHARED
`src/assoc.rs` is a **shared contract module**: it also exists (verbatim) in `webgen-settings`,
which renders the editor pane. Both read/write registry namespace **`com.webgen.OpensWith`**:
key = lowercase extension (no dot), value = launch command (argv0). An `@index` key lists the
extensions (the registry has no "list keys"). 1:1 mapping. Defaults seeded on first run
(`htm*`→webgen, `txt`/`csv`/`php`→webgen-edit, `sh`→webgen-terminal). **If you change the format
or namespace here, change it in `webgen-settings` too**, and note it in `webgen-distro/CONTRACT.md`.

## Settings
Scalar `show_hidden` (bool) via `webgen-registry` + `packaging/settings/com.webgen.Files.toml`,
rendered by System Settings. The associations *list* is not a manifest (System Settings has no
list widget) — it's a dedicated "Opens With" pane in webgen-settings.

## Packaging / shipping
`cargo deb` ships binary + desktop + manifest + icons + postinst cache-refresh. Push to
`git@github.com:piersrowan/webgen-files.git`, then set the `webgen-files` row's `wanted` note in
`../webgen-distro/INTEGRATION.md` for the OS session to pin. Don't emit/rsync/edit versions.tsv.

## Possible next work
Cut/move; rename; delete/trash; a per-row right-click menu with row-under-cursor selection;
drag-and-drop; an "Open With…" that also lists arbitrary installed apps (gio::AppInfo::all).
