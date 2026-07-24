//! Filesystem listing + recursive search, and the [`Entry`] rows the views display.
//!
//! Entries are plain Rust and travel inside a `glib::BoxedAnyObject`, so the GTK list models can
//! carry them without a hand-written GObject subclass.

use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::prelude::*;

/// Cap on recursive-search results, so a search from `/` can't run away. Reported when hit.
pub const SEARCH_CAP: usize = 2000;

/// One row in the file list: a file or folder, with the icon + labels the factory needs.
#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub icon: Option<gio::Icon>,
    /// For recursive results: the folder the match lives in, relative to the search root. Empty in
    /// a normal listing.
    pub subtitle: String,
}

/// Query attributes we need for each child in one enumerate call.
const ATTRS: &str =
    "standard::name,standard::display-name,standard::type,standard::icon,standard::is-hidden";

fn entry_from_info(dir: &Path, info: &gio::FileInfo, subtitle: String) -> Entry {
    let name = info.display_name().to_string();
    Entry {
        path: dir.join(info.name()),
        is_dir: info.file_type() == gio::FileType::Directory,
        icon: info.icon(),
        name,
        subtitle,
    }
}

/// List one directory: folders first, then files, each case-insensitively by name. Hidden entries
/// are included only when `show_hidden`. Returns an empty vec on any error (unreadable dir, etc.).
pub fn list_dir(dir: &Path, show_hidden: bool) -> Vec<Entry> {
    let file = gio::File::for_path(dir);
    let Ok(enumerator) =
        file.enumerate_children(ATTRS, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(info)) = enumerator.next_file(gio::Cancellable::NONE) {
        if info.is_hidden() && !show_hidden {
            continue;
        }
        out.push(entry_from_info(dir, &info, String::new()));
    }
    sort(&mut out);
    out
}

/// Recursively search `root` (and everything under it) for entries whose name contains `needle`
/// (case-insensitive). Breadth-first so nearer matches appear first; stops at [`SEARCH_CAP`]. The
/// `capped` flag tells the caller whether results were truncated.
pub fn search(root: &Path, needle: &str, show_hidden: bool) -> (Vec<Entry>, bool) {
    let needle = needle.to_lowercase();
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root.to_path_buf());
    let mut capped = false;

    while let Some(dir) = queue.pop_front() {
        let file = gio::File::for_path(&dir);
        let Ok(enumerator) =
            file.enumerate_children(ATTRS, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
        else {
            continue;
        };
        while let Ok(Some(info)) = enumerator.next_file(gio::Cancellable::NONE) {
            if info.is_hidden() && !show_hidden {
                continue;
            }
            let is_dir = info.file_type() == gio::FileType::Directory;
            let child = dir.join(info.name());
            if is_dir {
                queue.push_back(child.clone());
            }
            if info.display_name().to_lowercase().contains(&needle) {
                if out.len() >= SEARCH_CAP {
                    capped = true;
                    break;
                }
                let subtitle = dir
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_default();
                out.push(entry_from_info(&dir, &info, subtitle));
            }
        }
        if capped {
            break;
        }
    }
    sort(&mut out);
    (out, capped)
}

/// Folders before files, then case-insensitive by name.
fn sort(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
}

/// Copy `src` into directory `dest_dir`, recursing into folders and avoiding clobbering an existing
/// name (appends " (copy)"). Returns the created path, or an io error.
pub fn copy_into(src: &Path, dest_dir: &Path) -> std::io::Result<PathBuf> {
    let base = src.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let target = free_target(dest_dir, &base);
    copy_recursive(src, &target)?;
    Ok(target)
}

/// Move `src` into `dest_dir` (used by cut+paste). Fast `rename` within a filesystem, falling back to
/// copy-then-delete across devices. Non-clobbering, and a no-op if `src` is already in `dest_dir`.
pub fn move_into(src: &Path, dest_dir: &Path) -> std::io::Result<PathBuf> {
    if src.parent() == Some(dest_dir) {
        return Ok(src.to_path_buf());
    }
    let base = src.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let target = free_target(dest_dir, &base);
    match std::fs::rename(src, &target) {
        Ok(()) => Ok(target),
        Err(_) => {
            copy_recursive(src, &target)?;
            remove_path(src)?;
            Ok(target)
        }
    }
}

/// A non-clobbering path in `dest_dir` for name `base`: appends " (copy)" / " (copy N)" if taken.
fn free_target(dest_dir: &Path, base: &std::ffi::OsStr) -> PathBuf {
    let mut target = dest_dir.join(base);
    if target.exists() {
        let stem = Path::new(base).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let ext = Path::new(base).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let mut n = 1;
        loop {
            let suffix = if n == 1 { " (copy)".to_string() } else { format!(" (copy {})", n) };
            let candidate = dest_dir.join(format!("{stem}{suffix}{ext}"));
            if !candidate.exists() {
                target = candidate;
                break;
            }
            n += 1;
        }
    }
    target
}

/// Whether `name` looks like an archive we can extract (by extension). Used to grey out Compress vs
/// Extract in the right-click menu; the actual extraction is `bsdtar`, which reads all of these.
pub fn is_archive(name: &str) -> bool {
    let l = name.to_lowercase();
    const MULTI: &[&str] = &[".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tar.lz", ".tar.lzma"];
    const EXACT: &[&str] = &[
        ".zip", ".tar", ".tgz", ".tbz", ".tbz2", ".txz", ".tzst", ".7z", ".rar", ".jar", ".cbz",
        ".gz", ".bz2", ".xz", ".zst", ".lz", ".cpio", ".iso",
    ];
    MULTI.iter().any(|e| l.ends_with(e)) || EXACT.iter().any(|e| l.ends_with(e))
}

/// Delete `path`, recursively for a directory. A symlink is unlinked (never followed), so deleting
/// a link to a folder removes the link, not the folder's contents.
pub fn remove_path(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.file_type().is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn copy_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir(dst)?;
        for child in std::fs::read_dir(src)? {
            let child = child?;
            copy_recursive(&child.path(), &dst.join(child.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}
