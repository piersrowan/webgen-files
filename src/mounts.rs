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
//! SSH is **key only** -- no password field, nothing secret stored. The registry is plaintext
//! SQLite, so putting a share password in it would be handing it to anything that can read the
//! file.
//!
//! **SMB/CIFS** realistically needs a password, and was left out of the dialog until there was
//! somewhere safe to put one. `webgen-vault` is that place (2026-08-04): the password goes to
//! `webgen-vault add-login`, and the registry stores only the vault ENTRY NAME. Nothing secret
//! ever reaches the registry.
//!
//! mount(8) will not take a CIFS password except in argv (which `ps` shows to every local user,
//! and which sudo writes to auth.log verbatim) or in a credentials FILE. So at connect time the
//! password is written to a 0600 file under `$XDG_RUNTIME_DIR` -- a tmpfs the user alone can read
//! -- passed as `credentials=`, and deleted the moment mount returns, success or failure.

use std::path::PathBuf;

/// Registry key holding the comma-separated list of saved mount names. The registry is a flat
/// key/value store with no way to enumerate, so an index key is the established pattern here
/// (`assoc.rs` does the same for file associations).
const INDEX_KEY: &str = "@mounts";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Transport {
    Sshfs,
    Nfs,
    Cifs,
}

impl Transport {
    pub fn label(self) -> &'static str {
        match self {
            Transport::Sshfs => "SSH / SFTP",
            Transport::Nfs => "NFS",
            Transport::Cifs => "SMB / CIFS (Windows share)",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Transport::Sshfs => "sshfs",
            Transport::Nfs => "nfs",
            Transport::Cifs => "cifs",
        }
    }
    fn from_tag(s: &str) -> Option<Self> {
        match s {
            "sshfs" => Some(Transport::Sshfs),
            "nfs" => Some(Transport::Nfs),
            "cifs" => Some(Transport::Cifs),
            _ => None,
        }
    }
    /// Whether mounting this needs root. FUSE does not; the in-kernel filesystems do.
    pub fn needs_root(self) -> bool {
        matches!(self, Transport::Nfs | Transport::Cifs)
    }

    /// Whether this transport authenticates with a password (and therefore needs the vault).
    /// SSH uses keys and NFS trusts the network; only SMB asks for one.
    pub fn needs_password(self) -> bool {
        matches!(self, Transport::Cifs)
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
    /// Mount writable. **Defaults to false** -- a share is far more often browsed than edited,
    /// and a read-only mount cannot damage the far end by accident. Opt in when you mean it.
    pub writable: bool,
}

impl MountDef {
    /// Where it mounts. SSH mounts land under the user's own `~/mnt`, because FUSE mounts belong
    /// to the user and putting them in `/mnt` would need root for no reason. NFS is a system
    /// mount and goes in `/mnt`.
    pub fn target(&self) -> PathBuf {
        match self.transport {
            Transport::Sshfs => gtk::glib::home_dir().join("mnt").join(&self.name),
            // In-kernel mounts need root, so they live in /mnt like any other system mount.
            Transport::Nfs | Transport::Cifs => PathBuf::from("/mnt").join(&self.name),
        }
    }

    /// The mount option controlling writability. Both sshfs and mount(8) take ro/rw.
    pub fn rw_option(&self) -> &'static str {
        if self.writable { "rw" } else { "ro" }
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
            "{}\t{}\t{}\t{}\t{}\t{}",
            self.transport.tag(),
            self.host,
            self.port,
            self.user,
            self.remote,
            if self.writable { "rw" } else { "ro" }
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
            // Definitions saved before this field existed have five fields; treat them as
            // read-only, which is the safe reading of a missing value.
            writable: f.get(5).map(|v| *v == "rw").unwrap_or(false),
        })
    }

    /// The command that mounts this, as argv for `sh -c`-free execution.
    ///
    /// sshfs options worth keeping: `reconnect` and `ServerAliveInterval` mean a dropped link
    /// recovers instead of leaving a hung mount point, which is the usual complaint about sshfs.
    pub fn mount_argv(&self) -> Vec<String> {
        self.mount_argv_creds(None)
    }

    /// As [`mount_argv`], but with the path to a CIFS credentials file.
    ///
    /// SMB is the one transport that needs a secret at mount time, and mount(8) will only take it
    /// two ways: `-o password=...`, which `ps` shows to every local user and which sudo copies
    /// verbatim into auth.log, or `-o credentials=<file>`. So it is always the file. `creds` is
    /// `None` for the transports that need no secret, and a CIFS mount built without one is a
    /// programming error rather than a prompt -- it returns an argv that will simply fail to
    /// authenticate, which the caller surfaces as the mount error it is.
    pub fn mount_argv_creds(&self, creds: Option<&std::path::Path>) -> Vec<String> {
        let target = self.target().to_string_lossy().into_owned();
        match self.transport {
            Transport::Sshfs => vec![
                "sshfs".into(),
                "-p".into(),
                self.port.to_string(),
                "-o".into(),
                format!(
                    "{},reconnect,ServerAliveInterval=15,ServerAliveCountMax=3",
                    self.rw_option()
                ),
                format!("{}:{}", self.host_spec(), self.remote),
                target,
            ],
            Transport::Nfs => vec![
                "mount".into(),
                "-t".into(),
                "nfs4".into(),
                "-o".into(),
                self.rw_option().to_string(),
                format!("{}:{}", self.host, self.remote),
                target,
            ],
            Transport::Cifs => {
                // uid/gid: CIFS carries no Unix ownership on the wire for a plain SMB server, so
                // without these every file appears owned by root and the desktop cannot write.
                // Map the whole mount to whoever is running Files.
                let mut o = format!(
                    "{},uid={},gid={},iocharset=utf8,file_mode=0664,dir_mode=0775",
                    self.rw_option(),
                    // SAFETY: getuid/getgid cannot fail.
                    unsafe { libc::getuid() },
                    unsafe { libc::getgid() },
                );
                // vers=3.1.1 is the newest dialect. Not negotiating down to SMB1 is deliberate:
                // it is the protocol behind WannaCry, and ksmbd does not implement it at all.
                o.push_str(",vers=3.1.1");
                if let Some(c) = creds {
                    o.push_str(&format!(",credentials={}", c.to_string_lossy()));
                }
                vec![
                    "mount".into(),
                    "-t".into(),
                    "cifs".into(),
                    "-o".into(),
                    o,
                    // SMB addresses a share by NAME, not by server-side path -- //host/share.
                    format!("//{}/{}", self.host, self.remote.trim_start_matches('/')),
                    target,
                ]
            }
        }
    }

    /// The command that unmounts it. FUSE has its own unmounter that works unprivileged.
    pub fn umount_argv(&self) -> Vec<String> {
        let target = self.target().to_string_lossy().into_owned();
        match self.transport {
            Transport::Sshfs => vec!["fusermount3".into(), "-u".into(), target],
            Transport::Nfs | Transport::Cifs => vec!["umount".into(), target],
        }
    }
}

// ---------------------------------------------------------------------------------------------
// SMB credentials
//
// Nothing secret goes in the registry. The password lives in webgen-vault; the registry holds
// only the definition, and the vault entry name is derived from it. At mount time the password is
// written to a 0600 file under $XDG_RUNTIME_DIR (a per-user tmpfs, gone at logout) because
// mount(8) accepts a CIFS password no other safe way -- see mount_argv_creds.
// ---------------------------------------------------------------------------------------------

/// Vault entry name for a saved SMB connection. Namespaced so it cannot collide with an entry the
/// user created by hand for something else.
pub fn vault_entry(name: &str) -> String {
    format!("files-smb-{name}")
}

/// Read the saved password out of the vault. `None` if there is no entry, or the vault is locked
/// and cannot be unlocked without a passphrase prompt we have no terminal for.
pub fn vault_password(entry: &str) -> Option<String> {
    let out = std::process::Command::new("webgen-vault")
        .args(["get", entry, "--field", "password"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let p = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if p.is_empty() { None } else { Some(p) }
}

/// Save a login to the vault. The password goes in on STDIN -- `add-login` prompts for it, and
/// putting it in argv would show it in `ps` to every local user.
pub fn vault_save(entry: &str, user: &str, password: &str) -> Result<(), String> {
    use std::io::Write;
    let mut child = std::process::Command::new("webgen-vault")
        .args(["add-login", "--username", user, entry])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run webgen-vault: {e}"))?;
    {
        let si = child.stdin.as_mut().ok_or("no stdin for webgen-vault")?;
        // Twice: add-login asks for the password and then to confirm it.
        let _ = writeln!(si, "{password}");
        let _ = writeln!(si, "{password}");
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .lines()
            .last()
            .unwrap_or("webgen-vault refused the entry")
            .trim()
            .to_string())
    }
}

/// A credentials file that deletes itself.
///
/// Held only for the duration of one mount call. Drop removes it whether the mount succeeded,
/// failed, or panicked -- a leftover file containing a share password is exactly the thing this
/// whole arrangement exists to avoid.
pub struct Credentials(std::path::PathBuf);

impl Credentials {
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Write a CIFS credentials file, mode 0600, in the user's runtime directory.
///
/// $XDG_RUNTIME_DIR is a tmpfs owned by and readable only by this user, cleared at logout -- so
/// the password never touches persistent storage. /tmp is the fallback and is deliberately second
/// choice: it is world-traversable and survives a logout.
pub fn write_credentials(user: &str, password: &str) -> Result<Credentials, String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let dir = std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    let path = dir.join(format!("webgen-files-smb-{}", std::process::id()));

    let _ = std::fs::remove_file(&path);
    // 0600 at CREATE time via mode(), not chmod afterwards: a file that is briefly world-readable
    // between creation and chmod is a race, and the window is exactly when the secret is written.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("could not create the credentials file: {e}"))?;
    writeln!(f, "username={user}").map_err(|e| e.to_string())?;
    writeln!(f, "password={password}").map_err(|e| e.to_string())?;
    drop(f);
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    Ok(Credentials(path))
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
            writable: false,
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

#[cfg(test)]
mod rw_tests {
    use super::*;

    fn d(writable: bool) -> MountDef {
        MountDef {
            name: "share".into(),
            transport: Transport::Sshfs,
            host: "webgen.com.au".into(),
            port: 2222,
            user: String::new(),
            remote: "/var/www".into(),
            writable,
        }
    }

    #[test]
    fn read_only_is_the_default_and_reaches_the_command() {
        let argv = d(false).mount_argv().join(" ");
        assert!(argv.contains("ro,reconnect"), "read-only must be passed: {argv}");
        assert!(!argv.contains("rw,"), "must not mount writable when not asked");
    }

    #[test]
    fn writable_is_opt_in_and_reaches_the_command() {
        let argv = d(true).mount_argv().join(" ");
        assert!(argv.contains("rw,reconnect"), "read-write must be passed: {argv}");
    }

    #[test]
    fn nfs_carries_the_same_option() {
        let mut m = d(true);
        m.transport = Transport::Nfs;
        let argv = m.mount_argv();
        let i = argv.iter().position(|a| a == "-o").expect("-o must be present");
        assert_eq!(argv[i + 1], "rw");
    }

    #[test]
    fn writability_survives_a_save_and_load() {
        for w in [false, true] {
            let m = d(w);
            assert_eq!(MountDef::decode(&m.name, &m.encode()).unwrap().writable, w);
        }
    }

    #[test]
    fn a_definition_saved_before_this_field_existed_reads_as_read_only() {
        // Five-field records predate `writable`. Defaulting them to read-only is the safe
        // reading of a missing value -- the alternative silently makes an old mount writable.
        let old = "sshfs\twebgen.com.au\t2222\t\t/var/www";
        let parsed = MountDef::decode("share", old).expect("old records must still load");
        assert!(!parsed.writable);
    }
}


#[cfg(test)]
mod cifs_tests {
    use super::*;

    fn smb() -> MountDef {
        MountDef {
            name: "tv".into(),
            transport: Transport::Cifs,
            host: "192.168.1.200".into(),
            port: 445,
            user: "webgen".into(),
            remote: "share".into(),
            writable: true,
        }
    }

    #[test]
    fn smb_addresses_a_share_by_name_not_by_server_path() {
        // //host/share, never //host//home/share. Typing the server's directory here is the
        // commonest SMB mistake and produces a mount error that does not explain itself.
        let argv = smb().mount_argv_creds(None);
        assert!(argv.contains(&"//192.168.1.200/share".to_string()), "{argv:?}");
        // A leading slash on the share name must not double up.
        let mut d = smb();
        d.remote = "/share".into();
        assert!(d.mount_argv_creds(None).contains(&"//192.168.1.200/share".to_string()));
    }

    #[test]
    fn the_password_never_appears_in_argv() {
        // The whole reason credentials go in a file: ps shows argv to every local user, and sudo
        // copies it into auth.log.
        let creds = std::path::PathBuf::from("/run/user/1000/creds");
        let argv = smb().mount_argv_creds(Some(&creds)).join(" ");
        assert!(argv.contains("credentials=/run/user/1000/creds"), "{argv}");
        assert!(!argv.contains("password"), "{argv}");
    }

    #[test]
    fn smb_mounts_read_only_unless_asked() {
        let mut d = smb();
        d.writable = false;
        let argv = d.mount_argv_creds(None).join(" ");
        assert!(argv.contains("ro,"), "{argv}");
    }

    #[test]
    fn smb_needs_root_and_a_password_ssh_needs_neither() {
        assert!(Transport::Cifs.needs_root());
        assert!(Transport::Cifs.needs_password());
        assert!(!Transport::Sshfs.needs_root());
        assert!(!Transport::Sshfs.needs_password());
        // NFS needs root but has no password -- the server decides by address.
        assert!(Transport::Nfs.needs_root());
        assert!(!Transport::Nfs.needs_password());
    }

    #[test]
    fn cifs_survives_a_save_load_round_trip() {
        let d = smb();
        let restored = MountDef::decode("tv", &d.encode()).expect("decodes");
        assert_eq!(restored, d);
    }

    #[test]
    fn credentials_file_is_0600_and_deletes_itself() {
        use std::os::unix::fs::PermissionsExt;
        let path;
        {
            let c = write_credentials("webgen", "hunter2").expect("writes");
            path = c.path().to_path_buf();
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "credentials file must not be readable by anyone else");
            let body = std::fs::read_to_string(&path).unwrap();
            assert!(body.contains("username=webgen"));
            assert!(body.contains("password=hunter2"));
        }
        // Dropped -- the secret must not outlive the mount attempt.
        assert!(!path.exists(), "credentials file survived its guard");
    }
}
