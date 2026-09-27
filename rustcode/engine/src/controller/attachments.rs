//! Persist native frontend images where the existing multimodal prompt loader
//! can read them again when a saved session is resumed.

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(0);

/// Save an image from a native frontend and return the absolute path to use in
/// a `![image](file://...)` user prompt. The prompt loader supports these four
/// encoded formats without conversion.
pub fn save_image_attachment(bytes: &[u8], extension: &str) -> io::Result<PathBuf> {
    let config_dir = crate::config::get_config_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "RustCode config directory unavailable",
        )
    })?;
    save_image_in(&config_dir.join("attachments"), bytes, extension)
}

fn save_image_in(directory: &Path, bytes: &[u8], extension: &str) -> io::Result<PathBuf> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "image must be between 1 byte and 20 MiB",
        ));
    }
    let extension = extension.to_ascii_lowercase();
    if !matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "supported image formats: PNG, JPEG, GIF, WebP",
        ));
    }
    fs::create_dir_all(directory)?;
    let directory = fs::canonicalize(directory)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    for _ in 0..32 {
        let id = NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("native_{now}_{id}.{extension}"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes) {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(error);
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a unique attachment name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_unique_images_with_supported_extension() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = save_image_in(directory.path(), b"first", "PNG").expect("first image");
        let second = save_image_in(directory.path(), b"second", "png").expect("second image");
        assert_ne!(first, second);
        assert_eq!(
            first.extension().and_then(|part| part.to_str()),
            Some("png")
        );
        assert_eq!(fs::read(first).expect("saved bytes"), b"first");
        assert_eq!(fs::read(second).expect("saved bytes"), b"second");
    }

    #[test]
    fn rejects_unsupported_and_oversized_images() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(save_image_in(directory.path(), b"not an image", "svg").is_err());
        assert!(save_image_in(directory.path(), &[], "png").is_err());
        assert!(save_image_in(directory.path(), &vec![0; MAX_IMAGE_BYTES + 1], "png").is_err());
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("tempdir entries")
                .count(),
            0
        );
    }
}
