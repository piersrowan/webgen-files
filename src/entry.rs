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
    /// Byte size. `None` for folders, and for every entry when the details columns are off.
    ///
    /// Folders are deliberately left `None` rather than showing a recursive total: counting a
    /// directory tree means walking it, which turns listing a folder full of folders into
    /// something you wait for. Every fast file manager makes the same trade.
    pub size: Option<u64>,
    /// Owner and group names. Empty when the details columns are off.
    pub owner: String,
    pub group: String,
}

/// Query attributes we need for each child in one enumerate call.
const ATTRS: &str =
    "standard::name,standard::display-name,standard::type,standard::icon,standard::is-hidden";

/// The same, plus size and ownership, for when the details columns are shown.
///
/// These come from the SAME enumerate call rather than a `stat()` per entry -- GIO fetches them
/// in one pass, so turning the columns on costs one wider readdir rather than N syscalls.
const ATTRS_DETAIL: &str = "standard::name,standard::display-name,standard::type,standard::icon,\
    standard::is-hidden,standard::size,unix::uid,unix::gid";

fn entry_from_info_detail(
    dir: &Path,
    info: &gio::FileInfo,
    subtitle: String,
    ids: Option<&IdMap>,
) -> Entry {
    let name = info.display_name().to_string();
    let is_dir = info.file_type() == gio::FileType::Directory;
    let (size, owner, group) = match ids {
        None => (None, String::new(), String::new()),
        Some(ids) => (
            // Folders get no size on purpose -- see the field doc.
            (!is_dir).then(|| info.size() as u64),
            ids.user(info.attribute_uint32("unix::uid")),
            ids.group(info.attribute_uint32("unix::gid")),
        ),
    };
    Entry {
        path: dir.join(info.name()),
        is_dir,
        icon: info.icon(),
        name,
        subtitle,
        size,
        owner,
        group,
    }
}

/// uid/gid -> name, parsed ONCE per listing.
///
/// meta.rs resolves a single id by scanning /etc/passwd, which is fine for the one file an Info
/// dialog is about. Doing that per row would re-read and re-parse the whole file for every entry
/// in the folder, so the listing path builds the maps up front instead.
pub struct IdMap {
    users: std::collections::HashMap<u32, String>,
    groups: std::collections::HashMap<u32, String>,
}

impl IdMap {
    pub fn load() -> Self {
        IdMap {
            users: parse_ids("/etc/passwd"),
            groups: parse_ids("/etc/group"),
        }
    }
    fn user(&self, id: u32) -> String {
        self.users.get(&id).cloned().unwrap_or_else(|| id.to_string())
    }
    fn group(&self, id: u32) -> String {
        self.groups.get(&id).cloned().unwrap_or_else(|| id.to_string())
    }
}

/// `name:x:id:...` -- the shared shape of /etc/passwd and /etc/group.
fn parse_ids(file: &str) -> std::collections::HashMap<u32, String> {
    let mut map = std::collections::HashMap::new();
    if let Ok(content) = std::fs::read_to_string(file) {
        for line in content.lines() {
            let mut f = line.split(':');
            let (Some(name), Some(_), Some(id)) = (f.next(), f.next(), f.next()) else {
                continue;
            };
            if let Ok(id) = id.parse::<u32>() {
                map.entry(id).or_insert_with(|| name.to_string());
            }
        }
    }
    map
}

/// List one directory: folders first, then files, each case-insensitively by name. Hidden entries
/// are included only when `show_hidden`. Returns an empty vec on any error (unreadable dir, etc.).
pub fn list_dir(dir: &Path, show_hidden: bool, details: bool) -> Vec<Entry> {
    let ids = details.then(IdMap::load);
    let attrs = if details { ATTRS_DETAIL } else { ATTRS };
    let file = gio::File::for_path(dir);
    let Ok(enumerator) =
        file.enumerate_children(attrs, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(info)) = enumerator.next_file(gio::Cancellable::NONE) {
        if info.is_hidden() && !show_hidden {
            continue;
        }
        out.push(entry_from_info_detail(dir, &info, String::new(), ids.as_ref()));
    }
    sort_by(&mut out, SortKey::Name, false);
    out
}

/// Recursively search `root` (and everything under it) for entries whose name contains `needle`
/// (case-insensitive). Breadth-first so nearer matches appear first; stops at [`SEARCH_CAP`]. The
/// `capped` flag tells the caller whether results were truncated.
pub fn search(root: &Path, needle: &str, show_hidden: bool, details: bool) -> (Vec<Entry>, bool) {
    let ids = details.then(IdMap::load);
    let attrs = if details { ATTRS_DETAIL } else { ATTRS };
    let needle = needle.to_lowercase();
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(root.to_path_buf());
    let mut capped = false;

    while let Some(dir) = queue.pop_front() {
        let file = gio::File::for_path(&dir);
        let Ok(enumerator) =
            file.enumerate_children(attrs, gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
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
                out.push(entry_from_info_detail(&dir, &info, subtitle, ids.as_ref()));
            }
        }
        if capped {
            break;
        }
    }
    sort_by(&mut out, SortKey::Name, false);
    (out, capped)
}

/// What the list is ordered by. Only Name applies when the details columns are off, since the
/// other two have nothing to sort on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Owner,
}

/// Folders before files, then by `key`. Folders stay grouped first whatever the key -- that is
/// what every file manager does, and with no folder sizes a mixed size-sort would be meaningless.
/// Name is always the tie-break, so the order is stable and predictable.
pub fn sort_by(entries: &mut [Entry], key: SortKey, descending: bool) {
    entries.sort_by(|a, b| {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let ord = match key {
            SortKey::Name => by_name(),
            SortKey::Size => a.size.unwrap_or(0).cmp(&b.size.unwrap_or(0)).then_with(by_name),
            SortKey::Owner => a
                .owner
                .to_lowercase()
                .cmp(&b.owner.to_lowercase())
                .then_with(|| a.group.to_lowercase().cmp(&b.group.to_lowercase()))
                .then_with(by_name),
        };
        // Folders first is not reversed by the direction toggle -- flipping it would scatter
        // folders through the file list, which nobody wants.
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| if descending { ord.reverse() } else { ord })
    });
}

/// Human-readable byte size for the Size column. Folders show an em dash (see `Entry::size`).
pub fn human_size(size: Option<u64>) -> String {
    let Some(bytes) = size else {
        return "—".to_string();
    };
    const UNIT: f64 = 1024.0;
    if (bytes as f64) < UNIT {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB", "PB"];
    let (mut v, mut i) = (bytes as f64 / UNIT, 0);
    while v >= UNIT && i < units.len() - 1 {
        v /= UNIT;
        i += 1;
    }
    if v >= 100.0 { format!("{v:.0} {}", units[i]) } else { format!("{v:.1} {}", units[i]) }
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

#[cfg(test)]
mod detail_tests {
    use super::*;

    fn e(name: &str, is_dir: bool, size: Option<u64>, owner: &str, group: &str) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from(name),
            is_dir,
            icon: None,
            subtitle: String::new(),
            size,
            owner: owner.into(),
            group: group.into(),
        }
    }

    #[test]
    fn folders_stay_first_whatever_the_sort() {
        let mut v = vec![
            e("zeta.txt", false, Some(10), "piers", "users"),
            e("alpha", true, None, "root", "root"),
        ];
        for key in [SortKey::Name, SortKey::Size, SortKey::Owner] {
            for desc in [false, true] {
                sort_by(&mut v, key, desc);
                assert!(v[0].is_dir, "folders must lead for {key:?} desc={desc}");
            }
        }
    }

    #[test]
    fn size_sorts_numerically_not_as_text() {
        // The bug a naive string sort would give: "9 KB" after "10 KB".
        let mut v = vec![
            e("big", false, Some(9000), "a", "a"),
            e("small", false, Some(100), "a", "a"),
            e("mid", false, Some(1000), "a", "a"),
        ];
        sort_by(&mut v, SortKey::Size, false);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["small", "mid", "big"]);
        sort_by(&mut v, SortKey::Size, true);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["big", "mid", "small"]);
    }

    #[test]
    fn owner_sorts_then_group_then_name() {
        let mut v = vec![
            e("c", false, Some(1), "bob", "staff"),
            e("a", false, Some(1), "alice", "users"),
            e("b", false, Some(1), "alice", "admin"),
        ];
        sort_by(&mut v, SortKey::Owner, false);
        assert_eq!(v.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["b", "a", "c"]);
    }

    #[test]
    fn name_is_the_tie_break_so_order_is_stable() {
        let mut v = vec![
            e("b.txt", false, Some(5), "u", "g"),
            e("a.txt", false, Some(5), "u", "g"),
        ];
        sort_by(&mut v, SortKey::Size, false);
        assert_eq!(v[0].name, "a.txt");
    }

    #[test]
    fn folders_show_a_dash_not_a_size() {
        // Folders are never given a size -- counting a tree is what makes file managers slow.
        assert_eq!(human_size(None), "—");
        assert_eq!(human_size(Some(0)), "0 B");
        assert_eq!(human_size(Some(1024)), "1.0 KB");
        assert_eq!(human_size(Some(1024 * 1024 * 5)), "5.0 MB");
    }

    #[test]
    fn listing_without_details_carries_no_size_or_owner() {
        // Cheap mode must stay cheap: nothing populated, so the columns hide themselves.
        for entry in list_dir(Path::new("/"), false, false) {
            assert!(entry.size.is_none());
            assert!(entry.owner.is_empty() && entry.group.is_empty());
        }
    }

    #[test]
    fn listing_with_details_populates_owner_and_file_sizes() {
        let entries = list_dir(Path::new("/"), false, true);
        assert!(!entries.is_empty(), "/ should list something");
        for entry in &entries {
            assert!(!entry.owner.is_empty(), "{} has no owner", entry.name);
            assert!(!entry.group.is_empty(), "{} has no group", entry.name);
            // Folders deliberately have none; files must.
            if entry.is_dir {
                assert!(entry.size.is_none(), "{} is a folder and must have no size", entry.name);
            }
        }
    }

    #[test]
    fn ids_resolve_to_names_and_fall_back_to_the_number() {
        let ids = IdMap::load();
        assert_eq!(ids.user(0), "root", "uid 0 should resolve from /etc/passwd");
        // An id that cannot exist falls back to its number rather than being blank.
        assert_eq!(ids.user(4_294_967_294), "4294967294");
    }
}
