//! File metadata for the "Info" dialog: size, permissions (octal + rwx), owner/group, and dates.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

use gtk::glib;

pub struct Info {
    pub name: String,
    pub location: String,
    pub kind: String,
    pub size: String,
    pub perms_octal: String,
    pub perms_rwx: String,
    pub owner: String,
    pub group: String,
    pub modified: String,
    pub accessed: String,
    pub created: String,
}

/// Gather info for `path`. Uses `symlink_metadata` so a symlink reports its own type/perms rather
/// than its target's.
pub fn gather(path: &Path) -> std::io::Result<Info> {
    let md = std::fs::symlink_metadata(path)?;
    let ft = md.file_type();
    let kind = if ft.is_symlink() {
        "Symbolic link"
    } else if ft.is_dir() {
        "Folder"
    } else {
        "File"
    }
    .to_string();

    let size = if ft.is_dir() {
        match std::fs::read_dir(path) {
            Ok(rd) => {
                let n = rd.count();
                format!("{n} item{}", if n == 1 { "" } else { "s" })
            }
            Err(_) => "—".to_string(),
        }
    } else {
        human_size(md.len())
    };

    let mode = md.mode();
    Ok(Info {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string()),
        location: path
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default(),
        kind,
        size,
        perms_octal: format!("{:04o}", mode & 0o7777),
        perms_rwx: mode_to_rwx(mode),
        owner: resolve(md.uid(), "/etc/passwd"),
        group: resolve(md.gid(), "/etc/group"),
        modified: fmt_time(md.mtime()),
        accessed: fmt_time(md.atime()),
        // Birth time isn't available on every filesystem; show "—" when it isn't.
        created: md
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| fmt_time(d.as_secs() as i64))
            .unwrap_or_else(|| "—".to_string()),
    })
}

/// Format a Unix timestamp as local `YYYY-MM-DD HH:MM`.
fn fmt_time(secs: i64) -> String {
    glib::DateTime::from_unix_local(secs)
        .ok()
        .and_then(|d| d.format("%Y-%m-%d %H:%M").ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {} ({bytes} bytes)", UNITS[i])
}

/// e.g. mode 0o40755 → `drwxr-xr-x`.
fn mode_to_rwx(mode: u32) -> String {
    let type_ch = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        0o060000 => 'b',
        0o020000 => 'c',
        0o010000 => 'p',
        0o140000 => 's',
        _ => '-',
    };
    let mut s = String::with_capacity(10);
    s.push(type_ch);
    for &(bit, ch) in &[
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ] {
        s.push(if mode & bit != 0 { ch } else { '-' });
    }
    s
}

/// Resolve a uid/gid to `name (id)` by scanning `/etc/passwd` or `/etc/group` (fields
/// `name:x:id:...`). Falls back to the bare number.
fn resolve(id: u32, file: &str) -> String {
    if let Ok(content) = std::fs::read_to_string(file) {
        for line in content.lines() {
            let mut fields = line.split(':');
            let name = fields.next().unwrap_or("");
            let _passwd = fields.next();
            if fields.next().and_then(|f| f.parse::<u32>().ok()) == Some(id) {
                return format!("{name} ({id})");
            }
        }
    }
    id.to_string()
}
