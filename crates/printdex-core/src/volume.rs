//! Whether a library lives on a local disk, a removable drive or a network
//! share. Decides how the monitor notices changes (watching works on local
//! disks; shares are checked on a timer).

use std::path::{Path, PathBuf};

use crate::db::LibraryKind;

/// A mounted filesystem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    pub point: PathBuf,
    pub fs_type: String,
    pub removable: bool,
}

/// Filesystem types that are network shares (Linux and macOS names).
const NETWORK_TYPES: &[&str] = &[
    "cifs",
    "smb",
    "smb2",
    "smb3",
    "smbfs",
    "nfs",
    "nfs4",
    "afpfs",
    "afp",
    "webdav",
    "davfs",
    "9p",
    "ncpfs",
    "ceph",
    "glusterfs",
];

/// The kind of a mount by its filesystem type and removable flag.
pub fn classify_mount(fs_type: &str, removable: bool) -> LibraryKind {
    let t = fs_type.to_ascii_lowercase();
    let t = t.as_str();
    let fuse_network = t.strip_prefix("fuse.").is_some_and(|sub| {
        matches!(
            sub,
            "sshfs" | "rclone" | "davfs" | "smbnetfs" | "gvfsd-fuse"
        )
    });
    if NETWORK_TYPES.contains(&t) || t.starts_with("nfs") || t.starts_with("smb") || fuse_network {
        LibraryKind::Network
    } else if removable {
        LibraryKind::Removable
    } else {
        LibraryKind::Local
    }
}

/// The kind of `path` given the mounted filesystems: the mount with the
/// longest mount point that contains it decides. A Windows UNC path
/// (`\\server\share`) is always a share.
pub fn kind_for(path: &Path, mounts: &[Mount]) -> LibraryKind {
    let text = path.to_string_lossy();
    if text.starts_with(r"\\") && !text.starts_with(r"\\?\") || text.starts_with(r"\\?\UNC\") {
        return LibraryKind::Network;
    }
    mounts
        .iter()
        .filter(|m| path.starts_with(&m.point))
        .max_by_key(|m| m.point.components().count())
        .map_or(LibraryKind::Local, |m| {
            classify_mount(&m.fs_type, m.removable)
        })
}

/// The filesystems mounted right now. Blocking.
pub fn mounts() -> Vec<Mount> {
    let mut out: Vec<Mount> = match sysinfo::Disks::new_with_refreshed_list() {
        Ok(disks) => disks
            .iter()
            .map(|d| Mount {
                point: d.mount_point().to_path_buf(),
                fs_type: d.file_system().to_string_lossy().into_owned(),
                removable: d.is_removable(),
            })
            .collect(),
        Err(e) => {
            tracing::debug!(error = %e, "cannot list disks");
            Vec::new()
        }
    };
    // sysinfo leaves some network filesystems out on Linux; the kernel's
    // own list has them all.
    #[cfg(target_os = "linux")]
    if let Ok(text) = std::fs::read_to_string("/proc/self/mounts") {
        for m in parse_proc_mounts(&text) {
            if !out.iter().any(|o| o.point == m.point) {
                out.push(m);
            }
        }
    }
    out
}

/// Detects the kind of the volume `path` is on. Blocking.
pub fn detect(path: &Path) -> LibraryKind {
    kind_for(path, &mounts())
}

/// Parses `/proc/self/mounts` (`device point type options …`; spaces in
/// the mount point are written `\040`).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_mounts(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.split_ascii_whitespace();
            let _device = f.next()?;
            let point = f.next()?;
            let fs_type = f.next()?;
            Some(Mount {
                point: PathBuf::from(unescape_octal(point)),
                fs_type: fs_type.to_owned(),
                removable: false,
            })
        })
        .collect()
}

/// The byte an octal escape `\ooo` stands for; `None` when `digits` are not
/// three octal digits or the value does not fit a byte (`\777`).
fn octal_byte(digits: &[u8]) -> Option<u8> {
    if digits.len() != 3 {
        return None;
    }
    let mut v = 0u32;
    for &d in digits {
        if !(b'0'..=b'7').contains(&d) {
            return None;
        }
        v = v * 8 + u32::from(d - b'0');
    }
    u8::try_from(v).ok()
}

fn unescape_octal(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while let Some(&c) = b.get(i) {
        let escaped = if c == b'\\' {
            b.get(i + 1..i + 4).and_then(octal_byte)
        } else {
            None
        };
        if let Some(v) = escaped {
            out.push(v);
            i += 4;
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(point: &str, fs_type: &str, removable: bool) -> Mount {
        Mount {
            point: PathBuf::from(point),
            fs_type: fs_type.into(),
            removable,
        }
    }

    #[test]
    fn classify_mount_types() {
        for t in [
            "cifs",
            "smb3",
            "nfs",
            "nfs4",
            "afpfs",
            "fuse.sshfs",
            "davfs",
            "SMBFS",
        ] {
            assert_eq!(classify_mount(t, false), LibraryKind::Network, "{t}");
        }
        for t in ["ext4", "btrfs", "apfs", "ntfs", "fuse.portal", "zfs", "xfs"] {
            assert_eq!(classify_mount(t, false), LibraryKind::Local, "{t}");
        }
        assert_eq!(classify_mount("vfat", true), LibraryKind::Removable);
        // A share is a share even if the OS calls it removable.
        assert_eq!(classify_mount("cifs", true), LibraryKind::Network);
    }

    #[test]
    fn longest_mount_point_wins() {
        let mounts = [
            m("/", "ext4", false),
            m("/mnt/nas", "cifs", false),
            m("/mnt/nas/usb", "exfat", true),
        ];
        assert_eq!(
            kind_for(Path::new("/home/max/prints"), &mounts),
            LibraryKind::Local
        );
        assert_eq!(
            kind_for(Path::new("/mnt/nas/prints"), &mounts),
            LibraryKind::Network
        );
        assert_eq!(
            kind_for(Path::new("/mnt/nas/usb/x"), &mounts),
            LibraryKind::Removable
        );
        // Prefix by component, not by text.
        assert_eq!(
            kind_for(Path::new("/mnt/nasty"), &mounts),
            LibraryKind::Local
        );
    }

    #[test]
    fn unc_paths_are_shares() {
        assert_eq!(
            kind_for(Path::new(r"\\nas\prints"), &[]),
            LibraryKind::Network
        );
        assert_eq!(
            kind_for(Path::new(r"\\?\UNC\nas\prints"), &[]),
            LibraryKind::Network
        );
        assert_eq!(
            kind_for(Path::new(r"\\?\C:\prints"), &[]),
            LibraryKind::Local
        );
    }

    #[test]
    fn octal_escapes_that_overflow_a_byte_stay_literal() {
        assert_eq!(unescape_octal(r"/mnt/a\040b"), "/mnt/a b");
        // \777 is 511: not a byte, so it must not panic or wrap.
        assert_eq!(unescape_octal(r"/mnt/\777x"), r"/mnt/\777x");
        assert_eq!(unescape_octal(r"/mnt/\04"), r"/mnt/\04");
        assert_eq!(unescape_octal(r"/mnt/\"), r"/mnt/\");
    }

    #[test]
    fn parses_proc_mounts() {
        let text = "/dev/sda1 / ext4 rw 0 0\n//nas/prints /mnt/my\\040prints cifs rw 0 0\n";
        let mounts = parse_proc_mounts(text);
        assert_eq!(mounts[1], m("/mnt/my prints", "cifs", false));
        assert_eq!(
            kind_for(Path::new("/mnt/my prints/dragons"), &mounts),
            LibraryKind::Network
        );
    }

    #[test]
    fn detect_runs_on_this_machine() {
        // Whatever the temp dir is on, detection must not panic.
        let d = tempfile::tempdir().unwrap();
        let _ = detect(d.path());
    }
}
