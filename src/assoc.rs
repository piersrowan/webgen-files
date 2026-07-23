//! The shared **"Opens With"** file-association store.
//!
//! Associations are system-wide, so they live in their own registry namespace ([`NS`]) rather than
//! any one app's. Each row is 1:1 — one lowercase extension (no dot) → one launch command (an argv0
//! like `webgen`, `webgen-edit`, `webgen-terminal`, or any command on `PATH`). Both this file
//! manager and System Settings' "Opens With" pane read and write the same namespace and format, so
//! they stay in sync (see webgen-distro/CONTRACT.md §4).
//!
//! This is the shared association API; a few functions (`all`, `describe`, `clear`) are exercised
//! only by the Settings pane's copy of this module, so they read as dead code in this binary.
#![allow(dead_code)]

use webgen_registry::Registry;

/// Registry namespace holding the extension → command map. Not a real app id — a shared table.
pub const NS: &str = "com.webgen.OpensWith";

/// Index key: a comma-separated list of every extension that has ever had an association, so the
/// table can be enumerated through the registry's plain key/value API (which has no "list keys").
/// Both this app and System Settings maintain it identically, so they agree on the set of rows.
const INDEX: &str = "@index";

/// A known WebGen program, for showing an icon + friendly name next to a command. Unknown commands
/// still work — they just render with a generic icon and the raw command as their name.
pub struct Program {
    pub command: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
}

/// The WebGen family apps offered as open-with targets by default.
pub const PROGRAMS: &[Program] = &[
    Program { command: "webgen", name: "Browser", icon: "com.webgen.WebGen" },
    Program { command: "webgen-edit", name: "Edit", icon: "com.webgen.Edit" },
    Program { command: "webgen-terminal", name: "Terminal", icon: "com.webgen.Terminal" },
];

/// The sane defaults seeded on first run (matches the brief: htm*→Browser, txt/csv/php→Edit, sh→Terminal).
pub const DEFAULTS: &[(&str, &str)] = &[
    ("htm", "webgen"),
    ("html", "webgen"),
    ("xhtml", "webgen"),
    ("txt", "webgen-edit"),
    ("csv", "webgen-edit"),
    ("php", "webgen-edit"),
    ("sh", "webgen-terminal"),
];

/// The lowercase extension (no dot) of a file name, e.g. `Report.TXT` → `txt`. `None` if there is
/// no extension.
pub fn extension_of(name: &str) -> Option<String> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 >= name.len() {
        return None; // dotfile (".bashrc") or trailing dot — no usable extension
    }
    Some(name[dot + 1..].to_lowercase())
}

/// Look up the friendly name + icon for a command, falling back to the raw command for anything
/// outside [`PROGRAMS`].
pub fn describe(command: &str) -> (String, String) {
    for p in PROGRAMS {
        if p.command == command {
            return (p.name.to_string(), p.icon.to_string());
        }
    }
    (command.to_string(), "application-x-executable".to_string())
}

/// The command associated with an extension, if any.
pub fn program_for(reg: &Registry, ext: &str) -> Option<String> {
    let v = reg.get_string(NS, &ext.to_lowercase())?;
    if v.trim().is_empty() {
        None
    } else {
        Some(v)
    }
}

/// The extensions currently in the index (may include some whose value was cleared).
fn index(reg: &Registry) -> Vec<String> {
    reg.get_string(NS, INDEX)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Ensure `ext` is recorded in the index.
fn index_add(reg: &Registry, ext: &str) {
    let mut exts = index(reg);
    if !exts.iter().any(|e| e == ext) {
        exts.push(ext.to_string());
        exts.sort();
        let _ = reg.set_string(NS, INDEX, &exts.join(","));
    }
}

/// Associate (or re-associate) an extension with a command.
pub fn set_program(reg: &Registry, ext: &str, command: &str) {
    let ext = ext.to_lowercase();
    let _ = reg.set_string(NS, &ext, command);
    index_add(reg, &ext);
}

/// Remove an association (stores an empty string — the registry has no delete, and an empty value
/// reads back as "no association"). The extension stays in the index but [`all`] filters it out.
pub fn clear(reg: &Registry, ext: &str) {
    let _ = reg.set_string(NS, &ext.to_lowercase(), "");
}

/// Every non-empty association, sorted by extension.
pub fn all(reg: &Registry) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = index(reg)
        .into_iter()
        .filter_map(|ext| program_for(reg, &ext).map(|cmd| (ext, cmd)))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// Seed [`DEFAULTS`] the first time anyone opens the store, so a fresh system already knows how to
/// open web pages, text, and scripts. Only writes keys that are entirely absent, so it never
/// clobbers a user's edits or re-adds a row they cleared.
pub fn seed_defaults(reg: &Registry) {
    for (ext, command) in DEFAULTS {
        if reg.get_string(NS, ext).is_none() {
            set_program(reg, ext, command);
        }
    }
}
