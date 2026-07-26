//! Places: the mounted drives and network locations shown above the folder tree.
//!
//! Read from `/proc/mounts`, which is the kernel's own record -- `mount(8)` with no arguments
//! just prints it. Nothing else is authoritative, and it costs one file read.
//!
//! ## Grouping
//!
//! Local drives are listed individually. **Network mounts are grouped by host**, because that
//! is how people think about them: `webgen:/home/data`, `webgen:/var/www` and an sshfs of
//! `webgen:/home/backup` are three transports onto one machine, and once mounted the transport
//! stops mattering -- they are all just folders. So they collapse into a single "webgen" entry
//! containing three locations, and the host can be given a friendly name ("WebGen NFS Server")
//! that is remembered.
//!
//! Only the mount points themselves are listed, not the remote paths above them: you cannot
//! traverse above a mount point, so showing `/home/data` when all you can reach is the mounted
//! directory would be a lie.
//!
//! ## What is NOT here
//!
//! Mounting. WebGen has no udisks2/gvfs/polkit, so nothing auto-mounts removable media yet --
//! see the webgen-automount spec in webgen-distro/INTEGRATION.md. This module reports what is
//! mounted; it does not mount. Until the OS side lands, the Devices section will simply be
//! empty on a machine with a USB stick plugged in, which is the honest answer.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// How a place got here, which decides its icon and how it is grouped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A removable local drive -- USB stick, card reader, optical.
    Removable,
    /// A fixed local filesystem that is not the root one.
    Local,
    /// A network filesystem: nfs, cifs/smb, sshfs.
    Network,
}

#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub fstype: String,
    /// Host for network mounts; `None` for local ones.
    pub host: Option<String>,
}

/// Network filesystems we group by host. `fuse.sshfs` is matched by prefix.
const NET_FS: &[&str] = &["nfs", "nfs4", "cifs", "smb3", "smbfs", "afs", "ncpfs"];

/// Local filesystems worth showing as a place. Anything not here and not a network type is
/// assumed to be plumbing.
const LOCAL_FS: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3",
    "iso9660", "udf", "hfsplus",
];

fn is_network(fstype: &str) -> bool {
    NET_FS.contains(&fstype) || fstype.starts_with("fuse.sshfs") || fstype == "sshfs"
}

/// Everything mounted that a person would call a drive or a network location.
///
/// Excludes `/` and the pseudo-filesystems: the root filesystem already has a "Computer" entry
/// in the tree, and listing `proc`/`sysfs`/40 tmpfs mounts as "places" would be noise.
pub fn places() -> Vec<Place> {
    let Ok(text) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    let mut out: Vec<Place> = Vec::new();

    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(source), Some(target), Some(fstype)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let target = unescape(target);
        if target == "/" {
            continue;
        }

        let network = is_network(fstype);
        if !network && !LOCAL_FS.contains(&fstype) {
            continue;
        }
        // A local place must be a real block device; a network one never is.
        if !network && !source.starts_with("/dev/") {
            continue;
        }
        // Boot plumbing is not a "place" -- nobody browses their ESP.
        if !network && (target.starts_with("/boot") || target.starts_with("/run")) {
            continue;
        }
        if out.iter().any(|p| p.path == PathBuf::from(&target)) {
            continue;
        }

        let host = network.then(|| host_of(source)).flatten();
        let name = PathBuf::from(&target)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| target.clone());

        out.push(Place {
            kind: if network {
                Kind::Network
            } else if is_removable(source) {
                Kind::Removable
            } else {
                Kind::Local
            },
            name,
            path: PathBuf::from(&target),
            fstype: fstype.to_string(),
            host,
        });
    }
    out
}

/// The host a network mount source points at.
///
/// Handles the three shapes actually seen in /proc/mounts:
///   nfs    `webgen:/home/data`
///   cifs   `//webgen/share`
///   sshfs  `webgen:/home/backup`, or `user@webgen:/home/backup`
fn host_of(source: &str) -> Option<String> {
    let s = source.strip_prefix("//").unwrap_or(source);
    let head = if source.starts_with("//") {
        s.split('/').next()?
    } else {
        s.split(':').next()?
    };
    // Drop any user@ prefix -- "piers@webgen" and "webgen" are the same machine, and grouping
    // them apart would defeat the point.
    let head = head.rsplit('@').next()?;
    (!head.is_empty()).then(|| head.to_string())
}

/// Network places, grouped by host: one entry per machine, each with its mount points.
///
/// `rename` supplies the friendly name for a host, if the user has set one.
pub fn network_hosts(
    places: &[Place],
    rename: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String, Vec<Place>)> {
    let mut by_host: BTreeMap<String, Vec<Place>> = BTreeMap::new();
    for p in places.iter().filter(|p| p.kind == Kind::Network) {
        if let Some(h) = &p.host {
            by_host.entry(h.clone()).or_default().push(p.clone());
        }
    }
    by_host
        .into_iter()
        .map(|(host, mut mounts)| {
            mounts.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            let display = rename(&host).unwrap_or_else(|| host.clone());
            (host, display, mounts)
        })
        .collect()
}

/// Local drives (removable first, since those are the ones people came looking for).
pub fn local_drives(places: &[Place]) -> Vec<Place> {
    let mut v: Vec<Place> = places
        .iter()
        .filter(|p| p.kind != Kind::Network)
        .cloned()
        .collect();
    v.sort_by(|a, b| {
        (a.kind != Kind::Removable)
            .cmp(&(b.kind != Kind::Removable))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    v
}

pub fn icon_for(place: &Place) -> &'static str {
    match place.kind {
        Kind::Removable => "drive-removable-media-symbolic",
        Kind::Local => "drive-harddisk-symbolic",
        Kind::Network => "folder-remote-symbolic",
    }
}

/// `/proc/mounts` escapes space, tab, newline and backslash as octal.
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Whether the whole disk behind this partition is removable.
fn is_removable(source: &str) -> bool {
    let Some(dev) = source.strip_prefix("/dev/") else {
        return false;
    };
    let part = std::path::Path::new("/sys/class/block").join(dev);
    for c in [part.join("../removable"), part.join("removable")] {
        if let Ok(v) = std::fs::read_to_string(&c) {
            if v.trim() == "1" {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(name: &str, target: &str, host: &str, fstype: &str) -> Place {
        Place {
            name: name.into(),
            path: PathBuf::from(target),
            kind: Kind::Network,
            fstype: fstype.into(),
            host: Some(host.into()),
        }
    }

    #[test]
    fn host_is_extracted_from_every_source_shape() {
        assert_eq!(host_of("webgen:/home/data").as_deref(), Some("webgen"));
        assert_eq!(host_of("//webgen/share").as_deref(), Some("webgen"));
        assert_eq!(host_of("piers@webgen:/home/backup").as_deref(), Some("webgen"));
        assert_eq!(host_of("192.168.1.10:/export").as_deref(), Some("192.168.1.10"));
    }

    #[test]
    fn one_machine_reached_three_ways_groups_into_one_entry() {
        // The case from the design discussion: nfs /data, nfs /www, sshfs /backup on one host.
        let places = vec![
            net("data", "/mnt/data", "webgen", "nfs4"),
            net("www", "/mnt/www", "webgen", "nfs4"),
            net("backup", "/mnt/backup", "webgen", "fuse.sshfs"),
        ];
        let hosts = network_hosts(&places, |_| None);
        assert_eq!(hosts.len(), 1, "three mounts on one host must be one entry");
        let (host, display, mounts) = &hosts[0];
        assert_eq!(host, "webgen");
        assert_eq!(display, "webgen", "no rename set, so the host name shows");
        assert_eq!(mounts.len(), 3);
        // Transport is irrelevant once mounted -- all three sit together.
        assert_eq!(
            mounts.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            ["backup", "data", "www"]
        );
    }

    #[test]
    fn a_renamed_host_shows_its_friendly_name() {
        let places = vec![net("data", "/mnt/data", "webgen", "nfs4")];
        let hosts = network_hosts(&places, |h| {
            (h == "webgen").then(|| "WebGen NFS Server".to_string())
        });
        assert_eq!(hosts[0].1, "WebGen NFS Server");
        assert_eq!(hosts[0].0, "webgen", "the real host is kept as the key");
    }

    #[test]
    fn user_at_host_groups_with_the_bare_host() {
        // piers@webgen and webgen are the same machine.
        let places = vec![
            net("data", "/mnt/data", "webgen", "nfs4"),
            Place { host: host_of("piers@webgen:/home/backup"), ..net("backup", "/mnt/backup", "webgen", "fuse.sshfs") },
        ];
        assert_eq!(network_hosts(&places, |_| None).len(), 1);
    }

    #[test]
    fn removable_drives_sort_before_fixed_ones() {
        let p = |name: &str, kind| Place {
            name: name.into(), path: PathBuf::from(format!("/media/{name}")),
            kind, fstype: "vfat".into(), host: None,
        };
        let v = local_drives(&[p("archive", Kind::Local), p("USB STICK", Kind::Removable)]);
        assert_eq!(v[0].name, "USB STICK", "the drive you just plugged in comes first");
    }

    #[test]
    fn network_types_are_recognised_including_sshfs() {
        for fs in ["nfs", "nfs4", "cifs", "fuse.sshfs", "sshfs"] {
            assert!(is_network(fs), "{fs} should count as network");
        }
        for fs in ["ext4", "vfat", "btrfs"] {
            assert!(!is_network(fs));
        }
    }

    #[test]
    fn escaped_mount_points_are_decoded() {
        assert_eq!(unescape(r"/media/My\040Stick"), "/media/My Stick");
    }

    #[test]
    fn the_root_filesystem_and_plumbing_are_not_places() {
        // Runs against this machine's real /proc/mounts.
        for p in places() {
            assert_ne!(p.path, PathBuf::from("/"), "root has its own Computer entry");
            assert!(!p.path.starts_with("/proc") && !p.path.starts_with("/sys"));
            assert!(!p.path.starts_with("/boot"), "the ESP is not a place");
        }
    }
}
