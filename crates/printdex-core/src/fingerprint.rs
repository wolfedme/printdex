//! Partial-content fingerprints: file size plus xxh3-64 of the first and last
//! 64 KiB (the whole file when it is at most 128 KiB).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::fs::{Fs, RealFs};

use xxhash_rust::xxh3::Xxh3;

/// Bytes sampled from each end of a large file.
pub const SAMPLE_BYTES: u64 = SAMPLE_LEN as u64;
const SAMPLE_LEN: usize = 64 * 1024;

/// Cheap content identity of a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fingerprint {
    pub size: u64,
    pub hash: u64,
}

/// Fingerprints the file at `path`. Blocking; run it on the CPU pool.
pub fn fingerprint_file(path: &Path) -> std::io::Result<Fingerprint> {
    fingerprint_with(&RealFs, path)
}

/// Fingerprints `path` through `fs`.
pub fn fingerprint_with(fs: &dyn Fs, path: &Path) -> std::io::Result<Fingerprint> {
    let size = fs.metadata(path)?.len;
    let mut file = fs.open(path)?;
    let mut hasher = Xxh3::new();
    let mut buf = vec![0u8; SAMPLE_LEN];
    if size <= 2 * SAMPLE_BYTES {
        feed(&mut file, &mut hasher, &mut buf, size)?;
    } else {
        feed(&mut file, &mut hasher, &mut buf, SAMPLE_BYTES)?;
        file.seek(SeekFrom::Start(size - SAMPLE_BYTES))?;
        feed(&mut file, &mut hasher, &mut buf, SAMPLE_BYTES)?;
    }
    Ok(Fingerprint {
        size,
        hash: hasher.digest(),
    })
}

/// Feeds exactly `len` bytes from `file` into `hasher`. A file that shrank
/// since its size was read fails with `UnexpectedEof`.
#[expect(
    clippy::indexing_slicing,
    reason = "n is min(left, buf.len()), so n <= buf.len()"
)]
fn feed(file: &mut dyn Read, hasher: &mut Xxh3, buf: &mut [u8], len: u64) -> std::io::Result<()> {
    let mut left = len;
    while left > 0 {
        let n = usize::try_from(left.min(buf.len() as u64)).unwrap_or(buf.len());
        file.read_exact(&mut buf[..n])?;
        hasher.update(&buf[..n]);
        left -= n as u64;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use xxhash_rust::xxh3::xxh3_64;

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.path().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn pattern(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[test]
    fn small_file_hashes_whole_content() {
        let d = tempfile::tempdir().unwrap();
        let bytes = pattern(100);
        let fp = fingerprint_file(&write(&d, "a", &bytes)).unwrap();
        assert_eq!(fp.size, 100);
        assert_eq!(fp.hash, xxh3_64(&bytes));
    }

    #[test]
    fn boundary_128k_hashes_whole_content() {
        let d = tempfile::tempdir().unwrap();
        let bytes = pattern(131_072);
        let fp = fingerprint_file(&write(&d, "a", &bytes)).unwrap();
        assert_eq!(fp.hash, xxh3_64(&bytes));
    }

    #[test]
    fn large_file_hashes_head_and_tail_only() {
        let d = tempfile::tempdir().unwrap();
        let a = pattern(1 << 20);
        let mut b = a.clone();
        b[500_000] ^= 0xff;
        let mut c = a.clone();
        *c.last_mut().unwrap() ^= 0xff;
        let fa = fingerprint_file(&write(&d, "a", &a)).unwrap();
        let fb = fingerprint_file(&write(&d, "b", &b)).unwrap();
        let fc = fingerprint_file(&write(&d, "c", &c)).unwrap();
        assert_eq!(fa, fb);
        assert_ne!(fa.hash, fc.hash);
    }

    #[test]
    fn size_is_part_of_identity() {
        let d = tempfile::tempdir().unwrap();
        let head = vec![1u8; SAMPLE_BYTES as usize];
        let tail = vec![2u8; SAMPLE_BYTES as usize];
        let mut a = head.clone();
        a.extend(vec![0u8; 1000]);
        a.extend(&tail);
        let mut b = head.clone();
        b.extend(vec![0u8; 2000]);
        b.extend(&tail);
        let fa = fingerprint_file(&write(&d, "a", &a)).unwrap();
        let fb = fingerprint_file(&write(&d, "b", &b)).unwrap();
        assert_ne!(fa.size, fb.size);
        assert_ne!(fa, fb);
    }

    #[test]
    fn empty_file() {
        let d = tempfile::tempdir().unwrap();
        let fp = fingerprint_file(&write(&d, "a", b"")).unwrap();
        assert_eq!(fp.size, 0);
        assert_eq!(fp.hash, xxh3_64(b""));
    }

    #[test]
    fn fingerprint_of_missing_file_is_io_error() {
        let d = tempfile::tempdir().unwrap();
        let e = fingerprint_file(&d.path().join("nope")).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotFound);
    }
}
