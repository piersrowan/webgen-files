# webgen-files

A native GTK4 / libadwaita **file manager** for the WebGen app family (part of WebGen Linux).

## Features
- **Folder tree** on the left (toggle it on/off from the header), **files + subfolders** on the right
  with themed icons.
- **Navigate** with Back / Forward / Up, or type a path into the address bar.
- **New Folder**, **Copy / Paste** (recursive, never overwrites — makes `name (copy)`), **Open**, and
  **Open With…**.
- **Filter the current folder** as you type; tick **Recursive** to search every subfolder below the
  current location.
- **Open** honours the shared **"Opens With"** associations (edited in System Settings → Opens With,
  or set from *Open With… → Always open .ext this way*). Defaults out of the box: `htm*`→Browser,
  `txt`/`csv`/`php`→Edit, `sh`→Terminal. Falls back to the system default, then asks.

## Build / run
```sh
cargo run                    # needs libgtk-4-dev + libadwaita-1-dev
cargo build --release
cargo deb --no-build         # Debian package (binary, .desktop, manifest, icons, postinst)
```

Identity: appId / binary `com.webgen.Files` / `webgen-files`. Follows the shared `../webgen-distro`
contract + concentric-frame icon (teal folder). Settings (a `show_hidden` toggle) render in System
Settings from `packaging/settings/com.webgen.Files.toml`; associations live in the registry namespace
`com.webgen.OpensWith`.

## Shipping into WebGen Linux
Ships via the `wgpkg` package system. **You (this app's session) own the code; the OS/distro session
owns build, packaging, and publishing.** Full process: [`webgen-distro/INTEGRATION.md`](../webgen-distro/INTEGRATION.md).

When you want a change to ship:
1. Make the change; if user-visible, **bump the version in `Cargo.toml`** — clients' `wgpkg upgrade`
   only pulls a *higher* version.
2. **Commit and push.** The OS pins an immutable commit, so unpushed work can't ship.
3. In `webgen-distro/INTEGRATION.md`, set this app's row to **`wanted: <short-sha>`** with a one-line
   note (or tell Piers the commit).

You do **not** run `tools/emit-packages.sh`, edit `piers/config/versions.tsv`, or rsync — those are
OS-side. After you push, Piers tells the OS session to pin `webgen-files`; it builds → emits →
publishes, and clients receive it with `wgpkg update && wgpkg upgrade`.
