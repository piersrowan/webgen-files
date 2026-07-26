//! Saved mount definitions, and connecting/disconnecting them.
//!
//! A *definition* is what the user typed once ("the www share on webgen, over SSH"). It lives in
//! the registry and survives reboots. Whether it is currently **mounted** is a separate question,
//! answered by `/proc/mounts` (see `places.rs`) -- so a definition shows as connected or not
//! without us having to track state that the kernel already knows.
//!
//! ## Privileges
//!
//! **SSH/SFTP mounts need no root.** sshfs is FUSE, so it mounts as the ordinary user into their
//! own directory. NFS and SMB do need root and go through `sudo -n`, the same way the Services
//! and Storage panels in System Settings do.
//!
//! ## Credentials
//!
//! v1 is **SSH key only** -- no password field, nothing secret stored. That is not a shortcut:
//! the registry is plaintext SQLite, so putting a share password in it would be handing it to
//! anything that can read the file. Key-based SSH is also how these machines are actually
//! reached. SMB, which realistically needs a password, is therefore left out of the dialog until
//! there is somewhere safe to put one.

use std::path::PathBuf;

/// Registry key holding the comma-separated list of saved mount names. The registry is a flat
/// key/value store with no way to enumerate, so an index key is the established pattern here
/// (`assoc.rs` does the same for file associations).
const INDEX_KEY: &str = "@mounts";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Sshfs,
    Nfs,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Sshfs => "SSH / SFTP",
            Transport::Nfs => "NFS",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Transport::Sshfs => "sshfs",
            Transport::Nfs => "nfs",
        }
    }
    fn from_tag(s: &str) -> Option<Self> {
        match s {
            "sshfs" => Some(Transport::Sshfs),
            "nfs" => Some(Transport::Nfs),
            _ => None,
        }
    }
    /// Whether mounting this needs root. FUSE does not; the in-kernel filesystems do.
    pub fn needs_root(self) -> bool {
        matches!(self, Transport::Nfs)
    }
}

/// A saved connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountDef {
    /// Short name, also the folder name it mounts into. Unique; used as the registry key.
    pub name: String,
    pub transport: Transport,
    pub host: String,
    pub port: u16,
    /// SSH user. Empty means "whoever I am locally".
    pub user: String,
    pub remote: String,
}

impl MountDef {
    /// Where it mounts. SSH mounts land under the user's own `~/mnt`, because FUSE mounts belong
    /// to the user and putting them in `/mnt` would need root for no reason. NFS is a system
    /// mount and goes in `/mnt`.
    pub fn target(&self) -> PathBuf {
        match self.transport {
            Transport::Sshfs => gtk::glib::home_dir().join("mnt").join(&self.name),
            Transport::Nfs => PathBuf::from("/mnt").join(&self.name),
        }
    }

    /// `user@host` or just `host`.
    pub fn host_spec(&self) -> String {
        if self.user.trim().is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user.trim(), self.host)
        }
    }

    fn encode(&self) -> String {
        // Tab-separated: none of these fields can contain a tab, and it avoids inventing an
        // escaping scheme for a five-field record.
        format!(
            "{}\t{}\t{}\t{}\t{}",
            self.transport.tag(),
            self.host,
            self.port,
            self.user,
            self.remote
        )
    }

    fn decode(name: &str, s: &str) -> Option<Self> {
        let f: Vec<&str> = s.split('\t').collect();
        if f.len() < 5 {
            return None;
        }
        Some(MountDef {
            name: name.to_string(),
            transport: Transport::from_tag(f[0])?,
            host: f[1].to_string(),
            port: f[2].parse().ok()?,
            user: f[3].to_string(),
            remote: f[4].to_string(),
        })
    }

    /// The command that mounts this, as argv for `sh -c`-free execution.
    ///
    /// sshfs options worth keeping: `reconnect` and `ServerAliveInterval` mean a dropped link
    /// recovers instead of leaving a hung mount point, which is the usual complaint about sshfs.
    pub fn mount_argv(&self) -> Vec<String> {
        let target = self.target().to_string_lossy().into_owned();
        match self.transport {
            Transport::Sshfs => vec![
                "sshfs".into(),
                "-p".into(),
                self.port.to_string(),
                "-o".into(),
                "reconnect,ServerAliveInterval=15,ServerAliveCountMax=3".into(),
                format!("{}:{}", self.host_spec(), self.remote),
                target,
            ],
            Transport::Nfs => vec![
                "mount".into(),
                "-t".into(),
                "nfs4".into(),
                format!("{}:{}", self.host, self.remote),
                target,
            ],
        }
    }

    /// The command that unmounts it. FUSE has its own unmounter that works unprivileged.
    pub fn umount_argv(&self) -> Vec<String> {
        let target = self.target().to_string_lossy().into_owned();
        match self.transport {
            Transport::Sshfs => vec!["fusermount3".into(), "-u".into(), target],
            Transport::Nfs => vec!["umount".into(), target],
        }
    }
}

/// Every saved definition, in the order the index lists them.
pub fn load(reg: &crate::Reg) -> Vec<MountDef> {
    let Some(r) = reg.as_ref() else {
        return Vec::new();
    };
    let index = r.get_string(crate::FILES_NS, INDEX_KEY).unwrap_or_default();
    index
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .filter_map(|name| {
            r.get_string(crate::FILES_NS, &format!("mount:{name}"))
                .and_then(|v| MountDef::decode(name, &v))
        })
        .collect()
}

pub fn save(reg: &crate::Reg, def: &MountDef) {
    let Some(r) = reg.as_ref() else { return };
    let _ = r.set_string(crate::FILES_NS, &format!("mount:{}", def.name), &def.encode());
    let mut names: Vec<String> = load(reg).into_iter().map(|d| d.name).collect();
    if !names.iter().any(|n| n == &def.name) {
        names.push(def.name.clone());
    }
    let _ = r.set_string(crate::FILES_NS, INDEX_KEY, &names.join(","));
}

pub fn forget(reg: &crate::Reg, name: &str) {
    let Some(r) = reg.as_ref() else { return };
    let _ = r.set_string(crate::FILES_NS, &format!("mount:{name}"), "");
    let names: Vec<String> = load(reg)
        .into_iter()
        .map(|d| d.name)
        .filter(|n| n != name)
        .collect();
    let _ = r.set_string(crate::FILES_NS, INDEX_KEY, &names.join(","));
}

/// Whether this definition's target is currently mounted, according to the kernel.
pub fn is_connected(def: &MountDef) -> bool {
    let target = def.target();
    std::fs::read_to_string("/proc/mounts")
        .map(|t| {
            t.lines().any(|l| {
                l.split_whitespace()
                    .nth(1)
                    .map(|m| crate::places::unescape_public(m) == target.to_string_lossy())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Run a mount/unmount, returning the error text on failure.
///
/// Blocking -- call it off the UI thread. Root-needing transports are wrapped in `sudo -n`,
/// matching how the Services and Storage panels escalate.
pub fn run(argv: &[String], needs_root: bool) -> Result<(), String> {
    if argv.is_empty() {
        return Err("nothing to run".into());
    }
    let mut cmd = if needs_root {
        let mut c = std::process::Command::new("sudo");
        c.arg("-n");
        c.args(argv);
        c
    } else {
        let mut c = std::process::Command::new(&argv[0]);
        c.args(&argv[1..]);
        c
    };
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            Err(err
                .lines()
                .last()
                .unwrap_or("command failed")
                .trim()
                .to_string())
        }
        Err(e) => Err(format!("could not run {}: {e}", argv[0])),
    }
}

/// Create the mount point if it does not exist. sshfs and mount both refuse a missing target.
pub fn ensure_target(def: &MountDef) -> Result<(), String> {
    let target = def.target();
    if target.is_dir() {
        return Ok(());
    }
    if def.transport.needs_root() {
        run(
            &["mkdir".into(), "-p".into(), target.to_string_lossy().into_owned()],
            true,
        )
    } else {
        std::fs::create_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh() -> MountDef {
        MountDef {
            name: "webgen-site".into(),
            transport: Transport::Sshfs,
            host: "webgen.com.au".into(),
            port: 2222,
            user: "piers".into(),
            remote: "/var/www".into(),
        }
    }

    #[test]
    fn a_definition_survives_a_round_trip() {
        let d = ssh();
        assert_eq!(MountDef::decode(&d.name, &d.encode()).as_ref(), Some(&d));
    }

    #[test]
    fn garbage_decodes_to_nothing_rather_than_a_broken_entry() {
        assert!(MountDef::decode("x", "").is_none());
        assert!(MountDef::decode("x", "sshfs\thost").is_none());
        assert!(MountDef::decode("x", "carrier-pigeon\th\t22\tu\t/p").is_none());
    }

    #[test]
    fn sshfs_needs_no_root_but_nfs_does() {
        // The whole reason SSH mounts are pleasant: FUSE mounts as the user.
        assert!(!Transport::Sshfs.needs_root());
        assert!(Transport::Nfs.needs_root());
    }

    #[test]
    fn ssh_mounts_land_in_the_users_own_directory() {
        let t = ssh().target();
        assert!(t.starts_with(gtk::glib::home_dir()), "sshfs must not need /mnt");
        assert!(t.ends_with("webgen-site"));
        // NFS is a system mount.
        let mut n = ssh();
        n.transport = Transport::Nfs;
        assert_eq!(n.target(), PathBuf::from("/mnt/webgen-site"));
    }

    #[test]
    fn the_sshfs_command_carries_port_user_and_reconnect() {
        let argv = ssh().mount_argv();
        assert_eq!(argv[0], "sshfs");
        assert!(argv.contains(&"2222".to_string()), "non-default port must be passed");
        assert!(argv.iter().any(|a| a.contains("piers@webgen.com.au:/var/www")));
        // reconnect is what stops a dropped link leaving a hung mount point.
        assert!(argv.iter().any(|a| a.contains("reconnect")));
    }

    #[test]
    fn a_blank_user_means_the_local_one() {
        let mut d = ssh();
        d.user = "  ".into();
        assert_eq!(d.host_spec(), "webgen.com.au");
        assert!(d.mount_argv().iter().any(|a| a == "webgen.com.au:/var/www"));
    }

    #[test]
    fn fuse_is_unmounted_with_fusermount_not_umount() {
        // `umount` on a FUSE mount fails for an ordinary user; fusermount3 is the one that works.
        assert_eq!(ssh().umount_argv()[0], "fusermount3");
        let mut n = ssh();
        n.transport = Transport::Nfs;
        assert_eq!(n.umount_argv()[0], "umount");
    }

    #[test]
    fn connection_state_is_read_from_the_kernel_not_remembered() {
        // A definition pointing somewhere nothing is mounted must report disconnected, even
        // though the definition itself exists.
        let mut d = ssh();
        d.name = "definitely-not-mounted-xyz".into();
        assert!(!is_connected(&d));
    }
}
