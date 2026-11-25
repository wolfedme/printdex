//! Attempt photos: copied into Printdex's data folder, never left in the
//! library (v1 design §6, §8).

use std::path::Path;

use crate::error::{CoreError, Result};
use crate::ids::{AttemptId, PhotoId};

/// Largest photo accepted, in bytes.
pub const MAX_PHOTO_BYTES: u64 = 25 * 1024 * 1024;

/// The stored file extension for an image's bytes, by content (not name).
pub fn photo_extension(head: &[u8]) -> Option<&'static str> {
    match image::guess_format(head).ok()? {
        image::ImageFormat::Png => Some("png"),
        image::ImageFormat::Jpeg => Some("jpg"),
        image::ImageFormat::WebP => Some("webp"),
        _ => None,
    }
}

/// Copies `src` into `<photo_dir>/<attempt>/<photo>.<ext>`. Blocking.
/// Returns the stored file name.
pub fn store_photo(
    photo_dir: &Path,
    attempt: AttemptId,
    photo: PhotoId,
    src: &Path,
) -> Result<String> {
    use std::io::Read;
    let io = |source| CoreError::Io {
        path: src.to_path_buf(),
        source,
    };
    let meta = std::fs::metadata(src).map_err(io)?;
    if !meta.is_file() || meta.len() > MAX_PHOTO_BYTES {
        return Err(CoreError::PhotoInvalid);
    }
    let mut head = [0_u8; 32];
    let n = std::fs::File::open(src)
        .and_then(|mut f| f.read(&mut head))
        .map_err(io)?;
    let ext = photo_extension(head.get(..n).unwrap_or_default()).ok_or(CoreError::PhotoInvalid)?;
    let dir = photo_dir.join(attempt.to_string());
    std::fs::create_dir_all(&dir).map_err(|source| CoreError::Io {
        path: dir.clone(),
        source,
    })?;
    let name = format!("{photo}.{ext}");
    let dest = dir.join(&name);
    crate::fsx::copy_file_durable(src, &dest)
        .map_err(|source| CoreError::Io { path: dest, source })?;
    Ok(name)
}

/// Deletes stored photo files and empty attempt folders. Blocking; errors are
/// logged.
pub fn delete_files(paths: &[std::path::PathBuf]) {
    for p in paths {
        // Safe because callers pass only photos whose rows a delete (here or synced) already removed.
        if let Err(e) = std::fs::remove_file(p)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %p.display(), error = %e, "cannot delete photo");
        }
        if let Some(dir) = p.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbaImage::new(2, 2)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn stores_by_content_type() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("photo.jpeg");
        std::fs::write(&src, png()).unwrap();
        let (a, p) = (AttemptId::new(), PhotoId::new());
        let name = store_photo(&d.path().join("photos"), a, p, &src).unwrap();
        assert_eq!(name, format!("{p}.png"));
        assert!(
            d.path()
                .join("photos")
                .join(a.to_string())
                .join(&name)
                .is_file()
        );
    }

    #[test]
    fn rejects_non_images() {
        let d = tempfile::tempdir().unwrap();
        let src = d.path().join("notes.png");
        std::fs::write(&src, b"hello, not a png").unwrap();
        assert!(matches!(
            store_photo(d.path(), AttemptId::new(), PhotoId::new(), &src),
            Err(CoreError::PhotoInvalid)
        ));
    }
}
