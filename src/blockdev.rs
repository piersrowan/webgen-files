//! Block devices the machine can see, mounted or not — and mounting/unmounting them.
//!
//! ⚠ **SHARED CONTRACT MODULE.** This file also exists, verbatim, in `webgen-settings`
//! (`crates/webgen-settings/src/blockdev.rs`), which uses it for Settings → Storage. The two
//! must stay identical, exactly like `assoc.rs`. Change it in one, change it in the other, and
//! note it in `webgen-distro/CONTRACT.md`.
//!
//! ## Why this exists alongside `places.rs`
//!
//! `places.rs` reads `/proc/mounts`, which by definition only knows about things that are
//! **already mounted**. That leaves the most useful case invisible: a USB stick in the socket
//! that nothing has mounted, or a 747 GB Windows partition sitting unmounted on the internal
//! disk. This module enumerates what the machine can *see*, so those can be shown and offered.
//!
//! Consequence worth knowing: because a volume can be mounted from here, WebGen does **not**
//! strictly need an automounter for removable media to be usable — that becomes a convenience
//! rather than a prerequisite.
//!
//! ## Source
//!
//! `lsblk --json`, which works **unprivileged** and reports filesystem type, label, size,
//! mount point, removability and transport in one call. util-linux is in the base image, so
//! this costs no new package. Mounting itself needs root and goes through `sudo -n`, the same
//! escalation the Services and Storage panels already use.
//!
//! Because the two copies use different subsets of this API — Files only mounts, while Settings
//! also unmounts and needs the system-volume guard — a few methods read as dead code in each
//! binary. Same situation as `assoc.rs`, handled the same way.
#![allow(dead_code)]

use serde::Deserialize;

/// One mountable volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Volume {
    /// Kernel name, e.g. `sda1`.
    pub name: String,
    pub fstype: String,
    pub label: Option<String>,
    /// Human size as lsblk prints it, e.g. `7.5G`.
    pub size: String,
    /// `Some` when currently mounted.
    pub mountpoint: Option<String>,
    pub removable: bool,
    /// `usb`, `nvme`, `sata`… when lsblk knows.
    pub transport: Option<String>,
}

impl Volume {
    pub fn device(&self) -> String {
        format!("/dev/{}", self.name)
    }

    pub fn is_mounted(&self) -> bool {
        self.mountpoint.is_some()
    }

    /// What to call it: the volume label if it has one, else the device name.
    pub fn display_name(&self) -> String {
        match &self.label {
            Some(l) if !l.trim().is_empty() => l.clone(),
            _ => self.name.clone(),
        }
    }

    /// Volumes the system itself depends on. These are shown — they are real storage and the
    /// user should see their size — but are never offered an unmount action. Unmounting your
    /// own root filesystem should not be one click away.
    pub fn is_system(&self) -> bool {
        match self.mountpoint.as_deref() {
            Some("/") => true,
            Some(m) => m.starts_with("/boot") || m == "/usr" || m == "/var",
            None => false,
        }
    }

    /// Where an unmounted volume would be mounted. Matches the `webgen-automount` convention
    /// so a manually-mounted stick and an auto-mounted one land in the same place.
    pub fn mount_target(&self) -> String {
        let base = self.display_name();
        // A label can contain anything; a path component cannot contain '/' and should not
        // start with '.' (that would create a hidden mount point).
        let safe: String = base
            .chars()
            .map(|c| if c == '/' || c == '\\' { '-' } else { c })
            .collect();
        let safe = safe.trim().trim_start_matches('.');
        let safe = if safe.is_empty() { &self.name } else { safe };
        format!("/media/{safe}")
    }

    /// Command to mount this volume. Ownership options are applied only to filesystems that
    /// carry no Unix ownership of their own — without them a vfat/ntfs stick mounts root-owned
    /// and the desktop user cannot write to it. Applying them to ext4/btrfs would be wrong,
    /// because those have real ownership already.
    pub fn mount_argv(&self, uid: u32, gid: u32) -> Vec<String> {
        let mut a = vec!["mount".to_string()];
        let ownerless = matches!(self.fstype.as_str(), "vfat" | "exfat" | "ntfs" | "ntfs3" | "msdos");
        // nosuid,nodev on everything mounted this way: it may be a stick that arrived from
        // somewhere else, and neither setuid binaries nor device nodes on it are wanted.
        let mut opts = vec!["nosuid".to_string(), "nodev".to_string()];
        if ownerless {
            opts.push(format!("uid={uid}"));
            opts.push(format!("gid={gid}"));
            opts.push("fmask=0133".into());
            opts.push("dmask=0022".into());
        }
        a.push("-o".into());
        a.push(opts.join(","));
        a.push(self.device());
        a.push(self.mount_target());
        a
    }

    pub fn umount_argv(&self) -> Vec<String> {
        vec![
            "umount".to_string(),
            self.mountpoint.clone().unwrap_or_else(|| self.device()),
        ]
    }
}

// --- lsblk parsing ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LsblkRoot {
    blockdevices: Vec<Node>,
}

#[derive(Debug, Deserialize)]
struct Node {
    name: String,
    #[serde(default)]
    fstype: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    mountpoint: Option<String>,
    #[serde(default)]
    rm: Option<bool>,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    tran: Option<String>,
    #[serde(default)]
    children: Vec<Node>,
}

/// Every volume worth showing, mounted or not.
pub fn volumes() -> Vec<Volume> {
    let Ok(out) = std::process::Command::new("lsblk")
        .args(["--json", "-o", "NAME,FSTYPE,LABEL,SIZE,MOUNTPOINT,RM,TYPE,TRAN"])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

/// Split out from [`volumes`] so it can be tested against captured lsblk output.
pub fn parse(json: &str) -> Vec<Volume> {
    let Ok(root) = serde_json::from_str::<LsblkRoot>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in &root.blockdevices {
        walk(node, None, &mut out);
    }
    out.sort_by(|a, b| {
        // Removable first — the thing you just plugged in is what you came for — then
        // unmounted before mounted (they need an action), then by name.
        (!a.removable)
            .cmp(&(!b.removable))
            .then_with(|| a.is_mounted().cmp(&b.is_mounted()))
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn walk(node: &Node, parent_tran: Option<&str>, out: &mut Vec<Volume>) {
    let tran = node.tran.as_deref().or(parent_tran);
    let kind = node.kind.as_deref().unwrap_or("");
    let fstype = node.fstype.clone().unwrap_or_default();

    // Loop devices are snap/AppImage plumbing -- this machine has ~20 of them, and they would
    // swamp a list of "drives". Swap is not browsable. A node with no filesystem is a bare
    // disk or an extended-partition container: its children are the real volumes.
    let skip = kind == "loop"
        || fstype.is_empty()
        || fstype == "squashfs"
        || fstype == "swap"
        || fstype == "linux_raid_member"
        || fstype == "LVM2_member";

    if !skip {
        out.push(Volume {
            name: node.name.clone(),
            fstype,
            label: node.label.clone().filter(|l| !l.trim().is_empty()),
            size: node.size.clone().unwrap_or_default(),
            mountpoint: node.mountpoint.clone().filter(|m| !m.trim().is_empty()),
            removable: node.rm.unwrap_or(false),
            transport: tran.map(str::to_string),
        });
    }
    for child in &node.children {
        walk(child, tran, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `lsblk --json` output from the dev box, trimmed: a USB stick, the internal NVMe
    /// with ESP + an UNMOUNTED 747 GB NTFS partition + root, and a snap loop device.
    const SAMPLE: &str = r#"{
      "blockdevices": [
        {"name":"loop0","fstype":"squashfs","label":null,"size":"66.8M","mountpoint":"/snap/core24/1587","rm":false,"type":"loop","tran":null},
        {"name":"sda","fstype":null,"label":null,"size":"7.5G","mountpoint":null,"rm":true,"type":"disk","tran":"usb",
          "children":[
            {"name":"sda1","fstype":"vfat","label":"WGDIAG","size":"7.5G","mountpoint":null,"rm":true,"type":"part","tran":null}
          ]},
        {"name":"nvme0n1","fstype":null,"label":null,"size":"953.9G","mountpoint":null,"rm":false,"type":"disk","tran":"nvme",
          "children":[
            {"name":"nvme0n1p1","fstype":"vfat","label":"ESP","size":"250M","mountpoint":"/boot/efi","rm":false,"type":"part","tran":null},
            {"name":"nvme0n1p2","fstype":"ntfs","label":"OS","size":"747.4G","mountpoint":null,"rm":false,"type":"part","tran":null},
            {"name":"nvme0n1p3","fstype":"ext4","label":null,"size":"206.1G","mountpoint":"/","rm":false,"type":"part","tran":null}
          ]}
      ]}"#;

    fn vols() -> Vec<Volume> {
        parse(SAMPLE)
    }

    fn get<'a>(v: &'a [Volume], name: &str) -> &'a Volume {
        v.iter().find(|x| x.name == name).unwrap_or_else(|| panic!("{name} missing"))
    }

    #[test]
    fn the_unmounted_ntfs_partition_is_found() {
        // The whole point: a 747 GB Windows partition that /proc/mounts can never reveal.
        let v = vols();
        let ntfs = get(&v, "nvme0n1p2");
        assert_eq!(ntfs.fstype, "ntfs");
        assert_eq!(ntfs.label.as_deref(), Some("OS"));
        assert_eq!(ntfs.size, "747.4G");
        assert!(!ntfs.is_mounted());
        assert!(!ntfs.is_system(), "an unmounted data partition is not system-critical");
    }

    #[test]
    fn an_unmounted_usb_stick_is_found_and_inherits_its_transport() {
        let v = vols();
        let usb = get(&v, "sda1");
        assert!(usb.removable);
        assert!(!usb.is_mounted());
        assert_eq!(usb.display_name(), "WGDIAG", "the label is what people recognise");
        // tran is reported on the parent disk, not the partition.
        assert_eq!(usb.transport.as_deref(), Some("usb"));
    }

    #[test]
    fn loop_devices_and_bare_disks_are_excluded() {
        let v = vols();
        assert!(!v.iter().any(|x| x.name.starts_with("loop")), "snap loops would swamp the list");
        assert!(!v.iter().any(|x| x.name == "sda"), "a bare disk is not a volume");
        assert!(!v.iter().any(|x| x.name == "nvme0n1"));
        assert_eq!(v.len(), 4, "sda1, ESP, ntfs, root");
    }

    #[test]
    fn root_and_boot_are_system_and_must_not_offer_unmount() {
        let v = vols();
        assert!(get(&v, "nvme0n1p3").is_system(), "/ is system");
        assert!(get(&v, "nvme0n1p1").is_system(), "/boot/efi is system");
        assert!(!get(&v, "sda1").is_system());
    }

    #[test]
    fn removable_and_unmounted_sort_first() {
        // The stick you just plugged in should be at the top, not buried under the ESP.
        assert_eq!(vols()[0].name, "sda1");
    }

    #[test]
    fn ownerless_filesystems_get_uid_options_and_others_do_not() {
        let v = vols();
        let vfat = get(&v, "sda1").mount_argv(1000, 1000).join(" ");
        assert!(vfat.contains("uid=1000"), "vfat carries no ownership: {vfat}");
        assert!(vfat.contains("nosuid,nodev"), "removable media must be nosuid,nodev");

        // ext4 has real ownership -- forcing uid would be wrong.
        let mut ext = get(&v, "nvme0n1p3").clone();
        ext.mountpoint = None;
        let a = ext.mount_argv(1000, 1000).join(" ");
        assert!(!a.contains("uid="), "ext4 must not be given uid options: {a}");
        assert!(a.contains("nosuid,nodev"));
    }

    #[test]
    fn mount_target_uses_the_label_and_stays_a_safe_path() {
        let v = vols();
        assert_eq!(get(&v, "sda1").mount_target(), "/media/WGDIAG");
        assert_eq!(get(&v, "nvme0n1p2").mount_target(), "/media/OS");

        // A label is arbitrary user/vendor data, so the resulting path must be safe whatever
        // it says: one component under /media, no traversal, not hidden. Asserted as
        // properties rather than exact strings -- what matters is that none of these can
        // escape, not the precise mangling.
        let mut odd = get(&v, "sda1").clone();
        for label in ["../etc", ".hidden", "a/b", "..", "  ", "/"] {
            odd.label = Some(label.to_string());
            let target = odd.mount_target();
            let rest = target.strip_prefix("/media/").expect("must stay under /media");
            assert!(!rest.is_empty(), "{label:?} produced an empty mount point");
            assert!(!rest.contains('/'), "{label:?} escaped its component: {target}");
            assert!(!rest.starts_with('.'), "{label:?} produced a hidden mount point: {target}");
            assert!(!target.contains(".."), "{label:?} left a traversal in: {target}");
        }
        // An unlabelled volume falls back to its device name rather than "/media/".
        odd.label = None;
        assert_eq!(odd.mount_target(), "/media/sda1");
    }

    #[test]
    fn unmount_uses_the_mount_point_when_there_is_one() {
        let v = vols();
        let mut m = get(&v, "sda1").clone();
        m.mountpoint = Some("/media/WGDIAG".into());
        assert_eq!(m.umount_argv(), vec!["umount", "/media/WGDIAG"]);
    }

    #[test]
    fn broken_lsblk_output_yields_nothing_rather_than_panicking() {
        assert!(parse("not json").is_empty());
        assert!(parse(r#"{"blockdevices":[]}"#).is_empty());
    }

    #[test]
    fn this_machine_enumerates_without_error() {
        // Live check: whatever this box has, the shape must be self-consistent.
        for v in volumes() {
            assert!(!v.name.is_empty());
            assert!(!v.fstype.is_empty(), "{} has no fstype but was not filtered", v.name);
            assert!(v.device().starts_with("/dev/"));
        }
    }
}
