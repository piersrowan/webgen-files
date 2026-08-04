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

/// Same as [`unescape`], for callers outside this module (mounts.rs compares mount points).
pub fn unescape_public(s: &str) -> String {
    unescape(s)
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

// --- the sidebar widget ---------------------------------------------------------------------
//
// Kept apart from the model above so that half stays pure and unit-testable: nothing before
// this point touches GTK.

use adw::prelude::*;

/// Registry key holding the friendly name for a host, e.g. `hostname:webgen` -> "WebGen NFS
/// Server". Namespaced so it cannot collide with the app's own settings keys.
fn rename_key(host: &str) -> String {
    format!("hostname:{host}")
}

/// Build the Places section: Devices, then Network grouped by host.
///
/// `on_navigate` moves the file list. `reg` stores per-host friendly names. Returns the widget
/// and a closure that rebuilds it, so the caller can refresh after a mount or unmount.
pub fn build(
    reg: crate::Reg,
    on_navigate: impl Fn(std::path::PathBuf) + Clone + 'static,
) -> (gtk::Box, std::rc::Rc<dyn Fn()>) {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 0);

    // Fingerprint of the last-rendered mount set. Rebuilding on every tick would throw away
    // expander state and flicker, so the widget tree is only rebuilt when what is mounted
    // actually changes.
    let last = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
    // Lets the rebuild closure call itself after a connect/disconnect.
    #[allow(clippy::type_complexity)]
    let rebuild_ref: std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<dyn Fn()>>>> =
        std::rc::Rc::new(std::cell::RefCell::new(None));

    let rebuild: std::rc::Rc<dyn Fn()> = {
        let container = container.clone();
        let last = last.clone();
        let rebuild_ref = rebuild_ref.clone();
        let reg = reg.clone();
        std::rc::Rc::new(move || {
            let all = places();
            let fingerprint = all
                .iter()
                .map(|p| format!("{}\u{1}{}", p.path.display(), p.fstype))
                .collect::<Vec<_>>()
                .join("\u{2}");
            if *last.borrow() == fingerprint && container.first_child().is_some() {
                return;
            }
            *last.borrow_mut() = fingerprint;

            while let Some(c) = container.first_child() {
                container.remove(&c);
            }
            let drives = local_drives(&all);
            let reg2 = reg.clone();
            let hosts = network_hosts(&all, move |h| {
                reg2.as_ref()
                    .and_then(|r| r.get_string(crate::FILES_NS, &rename_key(h)))
                    .filter(|s| !s.trim().is_empty())
            });

            // Nothing mounted beyond the root filesystem is the normal state on WebGen today --
            // nothing auto-mounts removable media yet. Say so plainly rather than showing an
            // empty heading, which reads as broken.
            if drives.is_empty() && hosts.is_empty() && crate::blockdev::volumes().iter().all(|v| v.is_mounted()) {
                let hint = gtk::Label::new(Some("No drives or network locations mounted"));
                hint.add_css_class("dim-label");
                hint.add_css_class("caption");
                hint.set_wrap(true);
                hint.set_xalign(0.0);
                hint.set_margin_start(12);
                hint.set_margin_end(12);
                hint.set_margin_top(8);
                hint.set_margin_bottom(8);
                container.append(&hint);
                return;
            }

            // Volumes the machine can see but that are not mounted -- the USB stick nobody
            // mounted, the Windows partition on the internal disk. /proc/mounts can never show
            // these, and they are usually exactly what the user came looking for.
            let unmounted: Vec<crate::blockdev::Volume> = crate::blockdev::volumes()
                .into_iter()
                .filter(|v| !v.is_mounted())
                .collect();

            if !drives.is_empty() || !unmounted.is_empty() {
                container.append(&heading("Devices"));
                for d in &drives {
                    if d.kind == Kind::Removable {
                        let r = rebuild_ref.clone();
                        let after: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || {
                            if let Some(f) = r.borrow().as_ref() { f(); }
                        });
                        container.append(&mounted_row(d, on_navigate.clone(), after));
                    } else {
                        container.append(&place_row(d, on_navigate.clone()));
                    }
                }
                for v in &unmounted {
                    let r = rebuild_ref.clone();
                    let after: std::rc::Rc<dyn Fn()> = std::rc::Rc::new(move || {
                        if let Some(f) = r.borrow().as_ref() { f(); }
                    });
                    container.append(&unmounted_row(v, after));
                }
            }

            // Network heading carries the add-connection action.
            let net_head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let h = heading("Network");
            h.set_hexpand(true);
            net_head.append(&h);
            let add = gtk::Button::from_icon_name("list-add-symbolic");
            add.add_css_class("flat");
            add.set_valign(gtk::Align::End);
            add.set_tooltip_text(Some("Connect to a server"));
            add.set_margin_end(6);
            {
                let (reg, rebuild_again) = (reg.clone(), rebuild_ref.clone());
                add.connect_clicked(move |b| {
                    let after: std::rc::Rc<dyn Fn()> = {
                        let r = rebuild_again.clone();
                        std::rc::Rc::new(move || {
                            if let Some(f) = r.borrow().as_ref() {
                                f();
                            }
                        })
                    };
                    add_mount_dialog(b.root().and_downcast::<gtk::Window>().as_ref(), reg.clone(), after);
                });
            }
            net_head.append(&add);
            container.append(&net_head);

            if !hosts.is_empty() {
                for (host, display, mounts) in &hosts {
                    container.append(&host_row(
                        host,
                        display,
                        mounts,
                        reg.clone(),
                        on_navigate.clone(),
                        {
                            let r = rebuild_ref.clone();
                            std::rc::Rc::new(move || {
                                if let Some(f) = r.borrow().as_ref() { f(); }
                            })
                        },
                    ));
                }
            }

            // Saved connections that are not currently mounted, so they can be reconnected.
            // Whether something is mounted is read from the kernel, never remembered.
            let after: std::rc::Rc<dyn Fn()> = {
                let r = rebuild_ref.clone();
                std::rc::Rc::new(move || {
                    if let Some(f) = r.borrow().as_ref() {
                        f();
                    }
                })
            };
            for def in crate::mounts::load(&reg) {
                if !crate::mounts::is_connected(&def) {
                    container.append(&saved_row(&def, reg.clone(), after.clone()));
                }
            }
        })
    };
    // The rebuild closure needs to call itself (after connecting, the list must refresh), which
    // it cannot capture directly -- so it reaches itself through this cell.
    *rebuild_ref.borrow_mut() = Some(rebuild.clone());

    rebuild();
    (container, rebuild)
}

fn heading(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.add_css_class("dim-label");
    l.add_css_class("caption-heading");
    l.set_margin_start(12);
    l.set_margin_top(10);
    l.set_margin_bottom(2);
    l
}

/// The right way to unmount a given filesystem.
///
/// FUSE mounts (sshfs) must go through `fusermount3` -- plain `umount` fails for an ordinary
/// user. Everything else uses `umount`, which needs root.
fn umount_argv_for(p: &Place) -> (Vec<String>, bool) {
    let target = p.path.to_string_lossy().into_owned();
    if p.fstype.starts_with("fuse.") || p.fstype == "sshfs" {
        (vec!["fusermount3".into(), "-u".into(), target], false)
    } else {
        (vec!["umount".into(), target], true)
    }
}

/// A mounted place with a disconnect action. Works for anything mounted, not just connections
/// saved through this app -- if the kernel says it is mounted, it can be unmounted.
fn mounted_row(
    p: &Place,
    on_navigate: impl Fn(std::path::PathBuf) + 'static,
    after: std::rc::Rc<dyn Fn()>,
) -> gtk::Box {
    let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let button = place_row(p, on_navigate);
    button.set_hexpand(true);
    outer.append(&button);

    let eject = gtk::Button::from_icon_name("media-eject-symbolic");
    eject.add_css_class("flat");
    eject.set_valign(gtk::Align::Center);
    eject.set_tooltip_text(Some(&format!("Disconnect {}", p.name)));
    let (argv, needs_root) = umount_argv_for(p);
    let label = p.name.clone();
    eject.connect_clicked(move |b| {
        let (argv, after, label) = (argv.clone(), after.clone(), label.clone());
        let parent = b.root().and_downcast::<gtk::Window>();
        gtk::glib::spawn_future_local(async move {
            let a = argv.clone();
            let r = gtk::gio::spawn_blocking(move || crate::mounts::run(&a, needs_root)).await;
            match r {
                Ok(Ok(())) => after(),
                Ok(Err(msg)) => report(
                    parent.as_ref(),
                    &format!("Could not disconnect {label}"),
                    &msg,
                ),
                Err(_) => report(parent.as_ref(), "Could not disconnect", "the command did not run"),
            }
        });
    });
    outer.append(&eject);
    outer
}

/// One clickable drive or mount point.
fn place_row(p: &Place, on_navigate: impl Fn(std::path::PathBuf) + 'static) -> gtk::Button {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&gtk::Image::from_icon_name(icon_for(p)));
    let label = gtk::Label::new(Some(&p.name));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    label.set_hexpand(true);
    row.append(&label);

    let button = gtk::Button::new();
    button.set_child(Some(&row));
    button.add_css_class("flat");
    // Full path and filesystem type on hover -- the row itself stays short.
    button.set_tooltip_text(Some(&format!("{}  ({})", p.path.display(), p.fstype)));
    let path = p.path.clone();
    button.connect_clicked(move |_| on_navigate(path.clone()));
    button
}

/// A network host: one expandable entry containing its mount points, with a rename action.
///
/// This is the point of the whole module -- several transports onto one machine read as one
/// machine. The expander shows only the mount points, because you cannot traverse above them.
fn host_row(
    host: &str,
    display: &str,
    mounts: &[Place],
    reg: crate::Reg,
    on_navigate: impl Fn(std::path::PathBuf) + Clone + 'static,
    after: std::rc::Rc<dyn Fn()>,
) -> gtk::Expander {
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title.append(&gtk::Image::from_icon_name("computer-symbolic"));
    let label = gtk::Label::new(Some(display));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_hexpand(true);
    title.append(&label);

    let rename = gtk::Button::from_icon_name("document-edit-symbolic");
    rename.add_css_class("flat");
    rename.set_valign(gtk::Align::Center);
    rename.set_tooltip_text(Some("Rename this computer"));
    title.append(&rename);

    let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
    inner.set_margin_start(14);
    for m in mounts {
        inner.append(&mounted_row(m, on_navigate.clone(), after.clone()));
    }

    let expander = gtk::Expander::new(None);
    expander.set_label_widget(Some(&title));
    expander.set_child(Some(&inner));
    expander.set_expanded(true);

    // Rename: the friendly name is per host and remembered, so "webgen" can read
    // "WebGen NFS Server" without changing what is actually mounted.
    {
        let (host, label) = (host.to_string(), label.clone());
        rename.connect_clicked(move |btn| {
            // Resolve the window at click time by walking up from the button. The sidebar is
            // built before the window exists, so it cannot be captured here.
            let parent = btn.root().and_downcast::<gtk::Window>();
            let dialog = adw::MessageDialog::new(
                parent.as_ref(),
                Some("Rename computer"),
                Some(&format!("Shown instead of \"{host}\" in the sidebar.")),
            );
            let field = gtk::Entry::new();
            field.set_text(&label.text());
            field.set_margin_start(12);
            field.set_margin_end(12);
            field.set_margin_bottom(6);
            dialog.set_extra_child(Some(&field));
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("save", "Rename");
            dialog.set_default_response(Some("save"));
            dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);

            let (reg, host, label) = (reg.clone(), host.clone(), label.clone());
            dialog.connect_response(None, move |d, resp| {
                if resp == "save" {
                    let name = field.text().trim().to_string();
                    if let Some(r) = reg.as_ref() {
                        // Empty clears the override and falls back to the real host name.
                        let _ = r.set_string(crate::FILES_NS, &rename_key(&host), &name);
                    }
                    label.set_text(if name.is_empty() { &host } else { &name });
                }
                d.close();
            });
            dialog.present();
        });
    }

    expander
}

// --- saved connections: add, connect, disconnect ---------------------------------------------

/// The "Connect to Server" dialog. Saves the definition, then mounts it.
fn add_mount_dialog(
    parent: Option<&gtk::Window>,
    reg: crate::Reg,
    after: std::rc::Rc<dyn Fn()>,
) {
    use crate::mounts::{MountDef, Transport};

    let dialog = adw::MessageDialog::new(parent, Some("Connect to server"), None);
    let form = gtk::Box::new(gtk::Orientation::Vertical, 6);
    form.set_margin_start(12);
    form.set_margin_end(12);
    form.set_margin_bottom(6);

    let kind = gtk::DropDown::from_strings(&[
        Transport::Sshfs.label(),
        Transport::Nfs.label(),
        Transport::Cifs.label(),
    ]);
    let name = gtk::Entry::new();
    name.set_placeholder_text(Some("Name  (e.g. webgen-www)"));
    let host = gtk::Entry::new();
    host.set_placeholder_text(Some("Host  (e.g. webgen.com.au)"));
    let user = gtk::Entry::new();
    user.set_placeholder_text(Some("User  (optional — SSH only)"));
    let port = gtk::Entry::new();
    port.set_placeholder_text(Some("Port  (22 for SSH, blank for NFS)"));
    let remote = gtk::Entry::new();
    remote.set_placeholder_text(Some("Remote path  (e.g. /var/www)"));

    // SMB only. Hidden for SSH (keys) and NFS (no auth at all), so the form does not ask for a
    // secret it will not use.
    let password = gtk::PasswordEntry::new();
    password.set_show_peek_icon(true);
    password.set_placeholder_text(Some("Password  (SMB only)"));
    let remember = gtk::CheckButton::with_label("Remember this password in the vault");
    remember.set_active(true);

    for w in [&name, &host, &user, &port, &remote] {
        form.append(w);
    }
    form.append(&password);
    form.append(&remember);
    form.prepend(&kind);

    // Read-only by default. A share is browsed far more often than edited, and a read-only
    // mount cannot damage the far end by accident -- so writing is opt-in, not opt-out.
    let writable = gtk::CheckButton::with_label("Allow writing (default is read-only)");
    form.append(&writable);

    let note = gtk::Label::new(None);
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    note.set_wrap(true);
    note.set_xalign(0.0);
    form.append(&note);

    // The three transports want genuinely different things, and a form that shows all of it at
    // once is a form that asks SSH users for a password they do not have. Retitle and hide.
    let sync_form = {
        let (password, remember, note, remote, user, port) = (
            password.clone(),
            remember.clone(),
            note.clone(),
            remote.clone(),
            user.clone(),
            port.clone(),
        );
        move |sel: u32| {
            let is_smb = sel == 2;
            password.set_visible(is_smb);
            remember.set_visible(is_smb);
            port.set_visible(sel == 0);
            match sel {
                0 => {
                    remote.set_placeholder_text(Some("Remote path  (e.g. /var/www)"));
                    user.set_placeholder_text(Some("User  (optional)"));
                    note.set_text("SSH uses your existing key. Nothing secret is stored.");
                }
                1 => {
                    remote.set_placeholder_text(Some("Export path  (e.g. / for an fsid=0 export)"));
                    user.set_placeholder_text(Some("User  (unused for NFS)"));
                    note.set_text(
                        "NFS has no password: the server decides who may mount it, by address.",
                    );
                }
                _ => {
                    // SMB addresses a share by NAME, not by the server's directory path -- typing
                    // /home/share here is the commonest way to get "No such file or directory".
                    remote.set_placeholder_text(Some("Share name  (e.g. share — not a path)"));
                    user.set_placeholder_text(Some("User  (the SMB account)"));
                    note.set_text(
                        "The password is stored in webgen-vault, never in the file manager's \
                         settings. It is written to a temporary file only while mounting.",
                    );
                }
            }
        }
    };
    sync_form(0);
    kind.connect_selected_notify({
        let sync_form = sync_form.clone();
        move |k| sync_form(k.selected())
    });

    dialog.set_extra_child(Some(&form));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("connect", "Connect");
    dialog.set_default_response(Some("connect"));
    dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);

    dialog.connect_response(None, move |d, resp| {
        if resp != "connect" {
            d.close();
            return;
        }
        let transport = match kind.selected() {
            0 => Transport::Sshfs,
            1 => Transport::Nfs,
            _ => Transport::Cifs,
        };
        let def = MountDef {
            name: name.text().trim().replace('/', "-"),
            transport,
            host: host.text().trim().to_string(),
            port: port.text().trim().parse().unwrap_or(22),
            user: user.text().trim().to_string(),
            remote: remote.text().trim().to_string(),
            writable: writable.is_active(),
        };
        if def.name.is_empty() || def.host.is_empty() || def.remote.is_empty() {
            d.set_body("Name, host and remote path are all needed.");
            return;
        }
        // SMB authenticates as a named account; without a user there is nothing to authenticate,
        // and mount fails with a bare "permission denied" that says nothing about why.
        if transport.needs_password() && def.user.is_empty() {
            d.set_body("An SMB share needs the user it should connect as.");
            return;
        }
        let pw = password.text().to_string();
        if transport.needs_password() && pw.is_empty() {
            // Only an error if nothing is saved either -- reconnecting a known share should not
            // demand the password be retyped.
            if crate::mounts::vault_password(&crate::mounts::vault_entry(&def.name)).is_none() {
                d.set_body("This SMB share has no saved password. Enter it once and it will be kept in the vault.");
                return;
            }
        }
        // Store BEFORE mounting: if the mount fails for an unrelated reason (server down), the
        // credential is still saved and the retry does not ask again.
        if transport.needs_password() && !pw.is_empty() && remember.is_active() {
            if let Err(e) = crate::mounts::vault_save(
                &crate::mounts::vault_entry(&def.name),
                &def.user,
                &pw,
            ) {
                // Not fatal: mount with what was typed, and say the saving part failed. Refusing
                // to connect because the vault is locked would be worse than connecting once.
                d.set_body(&format!("Could not save to the vault ({e}). Connecting anyway."));
            }
        }
        d.close();
        crate::mounts::save(&reg, &def);
        let once = if pw.is_empty() { None } else { Some(pw) };
        connect(def, after.clone(), d.transient_for(), once);
    });
    dialog.present();
}

/// Mount a saved definition, off the UI thread, reporting failure in a dialog.
/// Mount a saved definition.
///
/// `password` is the one just typed, if any. When it is `None` and the transport needs one, the
/// vault is asked -- so a saved share reconnects without a prompt, which is the whole point of
/// storing it.
fn connect(
    def: crate::mounts::MountDef,
    after: std::rc::Rc<dyn Fn()>,
    parent: Option<gtk::Window>,
    password: Option<String>,
) {
    gtk::glib::spawn_future_local(async move {
        let d = def.clone();
        let result = gtk::gio::spawn_blocking(move || {
            crate::mounts::ensure_target(&d)?;
            if d.transport.needs_password() {
                let pw = password
                    .or_else(|| {
                        crate::mounts::vault_password(&crate::mounts::vault_entry(&d.name))
                    })
                    .ok_or_else(|| {
                        "no password for this share, and the vault did not supply one \
                         (is it locked? run `webgen-vault unlock`)"
                            .to_string()
                    })?;
                // The file is deleted when `creds` drops -- at the end of this block, whether the
                // mount worked or not.
                let creds = crate::mounts::write_credentials(&d.user, &pw)?;
                crate::mounts::run(&d.mount_argv_creds(Some(creds.path())), true)
            } else {
                crate::mounts::run(&d.mount_argv(), d.transport.needs_root())
            }
        })
        .await;

        match result {
            Ok(Ok(())) => after(),
            Ok(Err(msg)) => report(parent.as_ref(), &format!("Could not connect {}", def.name), &msg),
            Err(_) => report(parent.as_ref(), "Could not connect", "the mount command did not run"),
        }
    });
}

/// Unmount, then refresh.
fn disconnect(
    def: crate::mounts::MountDef,
    after: std::rc::Rc<dyn Fn()>,
    parent: Option<gtk::Window>,
) {
    gtk::glib::spawn_future_local(async move {
        let d = def.clone();
        let result = gtk::gio::spawn_blocking(move || {
            crate::mounts::run(&d.umount_argv(), d.transport.needs_root())
        })
        .await;
        match result {
            Ok(Ok(())) => after(),
            // "Device or resource busy" is the common one and the user can act on it.
            Ok(Err(msg)) => report(parent.as_ref(), &format!("Could not disconnect {}", def.name), &msg),
            Err(_) => report(parent.as_ref(), "Could not disconnect", "the command did not run"),
        }
    });
}

fn report(parent: Option<&gtk::Window>, heading: &str, body: &str) {
    let d = adw::MessageDialog::new(parent, Some(heading), Some(body));
    d.add_response("ok", "OK");
    d.present();
}

/// Saved connections that are not currently mounted, shown so they can be reconnected.
fn saved_row(
    def: &crate::mounts::MountDef,
    reg: crate::Reg,
    after: std::rc::Rc<dyn Fn()>,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = gtk::Image::from_icon_name("folder-remote-symbolic");
    icon.set_opacity(0.45);
    row.append(&icon);
    let label = gtk::Label::new(Some(&def.name));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_hexpand(true);
    label.add_css_class("dim-label");
    row.append(&label);

    let connect_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
    connect_btn.add_css_class("flat");
    connect_btn.set_valign(gtk::Align::Center);
    connect_btn.set_tooltip_text(Some(&format!(
        "Connect to {}:{}  ({})",
        def.host_spec(),
        def.remote,
        if def.writable { "read-write" } else { "read-only" }
    )));
    {
        let (def, after) = (def.clone(), after.clone());
        connect_btn.connect_clicked(move |b| {
            // No password passed: a saved share pulls it from the vault.
            connect(def.clone(), after.clone(), b.root().and_downcast::<gtk::Window>(), None);
        });
    }
    row.append(&connect_btn);

    let forget_btn = gtk::Button::from_icon_name("user-trash-symbolic");
    forget_btn.add_css_class("flat");
    forget_btn.set_valign(gtk::Align::Center);
    forget_btn.set_tooltip_text(Some("Forget this connection"));
    {
        let (name, reg, after) = (def.name.clone(), reg.clone(), after.clone());
        forget_btn.connect_clicked(move |_| {
            crate::mounts::forget(&reg, &name);
            after();
        });
    }
    row.append(&forget_btn);
    row
}

/// A volume the machine can see but has not mounted: greyed, with a mount action.
///
/// Mounting is behind a confirmation because it is a real operation on real hardware -- it can
/// spin up a disk, replay a dirty journal, or expose a filesystem that arrived from somewhere
/// else. A single stray click should not do that.
fn unmounted_row(v: &crate::blockdev::Volume, after: std::rc::Rc<dyn Fn()>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let icon = gtk::Image::from_icon_name(if v.removable {
        "drive-removable-media-symbolic"
    } else {
        "drive-harddisk-symbolic"
    });
    icon.set_opacity(0.45);
    row.append(&icon);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&v.display_name()));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    name.add_css_class("dim-label");
    let detail = gtk::Label::new(Some(&format!("{}  ·  {}  ·  not mounted", v.size, v.fstype)));
    detail.set_xalign(0.0);
    detail.add_css_class("dim-label");
    detail.add_css_class("caption");
    text.append(&name);
    text.append(&detail);
    row.append(&text);

    let mount = gtk::Button::from_icon_name("media-playback-start-symbolic");
    mount.add_css_class("flat");
    mount.set_valign(gtk::Align::Center);
    mount.set_tooltip_text(Some(&format!("Mount {} at {}", v.device(), v.mount_target())));

    let vol = v.clone();
    mount.connect_clicked(move |b| {
        let parent = b.root().and_downcast::<gtk::Window>();
        let dialog = adw::MessageDialog::new(
            parent.as_ref(),
            Some(&format!("Mount {}?", vol.display_name())),
            Some(&format!(
                "{} ({}, {}) will be mounted at {}.",
                vol.device(),
                vol.fstype,
                vol.size,
                vol.mount_target()
            )),
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("mount", "Mount");
        dialog.set_default_response(Some("mount"));
        dialog.set_response_appearance("mount", adw::ResponseAppearance::Suggested);

        let (vol, after) = (vol.clone(), after.clone());
        dialog.connect_response(None, move |d, resp| {
            d.close();
            if resp != "mount" {
                return;
            }
            let (vol, after) = (vol.clone(), after.clone());
            let parent = d.transient_for();
            gtk::glib::spawn_future_local(async move {
                let v2 = vol.clone();
                let r = gtk::gio::spawn_blocking(move || {
                    // The mount point has to exist first, and creating it under /media needs
                    // root -- so it goes through the same sudo -n call as the mount.
                    let mkdir = vec!["mkdir".to_string(), "-p".into(), v2.mount_target()];
                    crate::mounts::run(&mkdir, true)?;
                    let uid = unsafe { libc::getuid() };
                    let gid = unsafe { libc::getgid() };
                    crate::mounts::run(&v2.mount_argv(uid, gid), true)
                })
                .await;
                match r {
                    Ok(Ok(())) => after(),
                    Ok(Err(msg)) => report(
                        parent.as_ref(),
                        &format!("Could not mount {}", vol.display_name()),
                        &msg,
                    ),
                    Err(_) => report(parent.as_ref(), "Could not mount", "the command did not run"),
                }
            });
        });
        dialog.present();
    });
    row.append(&mount);
    row
}
