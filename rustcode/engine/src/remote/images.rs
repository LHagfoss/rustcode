//! Private, authenticated image attachments. Uploading never submits a prompt.
use std::{
    fs::{self, OpenOptions},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::protocol::RemoteResult;
use crate::daemon::lifecycle::{
    ensure_private_directory, lock_private_file, publish_private_json, read_private_json,
};

pub(super) const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 128 * 1024;
const MAX_SESSION_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ATTACHMENTS: usize = 256;
const PARTIAL_TTL: Duration = Duration::from_secs(3600);
const MARKER: &str = "![image](file://";

#[derive(Serialize, Deserialize)]
struct Metadata {
    mime_type: String,
    total_bytes: u64,
    created: u64,
    complete: bool,
}

pub(super) fn root(session_id: &str) -> Result<PathBuf> {
    Ok(crate::config::get_active_session_artifacts_dir(session_id)
        .context("session storage is unavailable")?
        .join("remote-images"))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn device_directory(root: &Path, device: &str) -> PathBuf {
    root.join(format!("{:x}", Sha256::digest(device.as_bytes())))
}

fn extension(mime: &str) -> Result<&'static str> {
    match mime {
        "image/jpeg" => Ok("jpg"),
        "image/png" => Ok("png"),
        _ => anyhow::bail!("only JPEG and PNG images are supported"),
    }
}

fn private_file(path: &Path, write: bool) -> Result<std::fs::File> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .create(write)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "attachment must be a regular file"
    );
    Ok(file)
}

/// Delete only abandoned partial uploads; completed files remain with history.
fn reserved_bytes(root: &Path) -> Result<(u64, usize)> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut reserved = 0u64;
    let mut count = 0;
    for device in fs::read_dir(root)? {
        let device = device?;
        if !device.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(device.path())? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let metadata: Metadata = read_private_json(&path, 2048)?;
            if !metadata.complete && now.saturating_sub(metadata.created) > PARTIAL_TTL.as_secs() {
                let data = path.with_extension(extension(&metadata.mime_type)?);
                if data.exists() {
                    fs::remove_file(data)?;
                }
                fs::remove_file(path)?;
                continue;
            }
            reserved = reserved
                .checked_add(metadata.total_bytes)
                .context("image budget exceeded")?;
            count += 1;
        }
    }
    Ok((reserved, count))
}

fn validate_image(bytes: &[u8], mime: &str) -> Result<()> {
    let expected = match mime {
        "image/png" => image::ImageFormat::Png,
        _ => image::ImageFormat::Jpeg,
    };
    ensure!(
        image::guess_format(bytes)? == expected,
        "image content does not match its MIME type"
    );
    let mut reader = image::ImageReader::with_format(Cursor::new(bytes), expected);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    reader
        .decode()
        .context("image is invalid or exceeds 4096 × 4096 pixels")?;
    Ok(())
}

pub(super) fn upload(
    root: &Path,
    device: &str,
    operation: &super::RemoteOperation,
) -> Result<RemoteResult> {
    let super::RemoteOperation::UploadImage {
        attachment_id,
        mime_type,
        offset,
        total_bytes,
        data_base64,
    } = operation
    else {
        anyhow::bail!("expected an image upload")
    };
    ensure!(
        !device.is_empty(),
        "image uploads require an authenticated device"
    );
    ensure!(
        valid_id(attachment_id),
        "attachment_id must be 1–64 ASCII letters, digits, '-' or '_'"
    );
    let ext = extension(mime_type)?;
    ensure!(
        *total_bytes > 0 && *total_bytes <= MAX_IMAGE_BYTES,
        "images must be between 1 byte and 8 MiB"
    );
    ensure!(
        data_base64.len() <= MAX_CHUNK_BYTES.div_ceil(3) * 4,
        "image chunks must not exceed 128 KiB"
    );
    let bytes = STANDARD
        .decode(data_base64)
        .context("invalid image base64")?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_CHUNK_BYTES,
        "image chunk must contain 1–131072 bytes"
    );
    let next = offset
        .checked_add(bytes.len() as u64)
        .context("invalid image offset")?;
    ensure!(next <= *total_bytes, "image chunk exceeds declared size");
    ensure_private_directory(root, "image attachments")?;
    let root = root.canonicalize()?;
    ensure!(
        !root.to_string_lossy().contains(')'),
        "session storage path cannot contain ')' for image attachments"
    );
    let _lock = lock_private_file(&root, "upload.lock", "image attachments")?;
    let (reserved, count) = reserved_bytes(&root)?;
    let directory = device_directory(&root, device);
    ensure_private_directory(&directory, "device image attachments")?;
    let meta_path = directory.join(format!("{attachment_id}.json"));
    let data_path = directory.join(format!("{attachment_id}.{ext}"));
    let mut metadata = if meta_path.exists() {
        let metadata: Metadata = read_private_json(&meta_path, 2048)?;
        ensure!(
            metadata.mime_type == *mime_type && metadata.total_bytes == *total_bytes,
            "attachment_id already has different image metadata"
        );
        metadata
    } else {
        ensure!(*offset == 0, "start an image upload at offset 0");
        ensure!(
            reserved + total_bytes <= MAX_SESSION_BYTES && count < MAX_ATTACHMENTS,
            "session image storage is full (64 MiB or 256 attachments); start a new session"
        );
        let metadata = Metadata {
            mime_type: mime_type.clone(),
            total_bytes: *total_bytes,
            created: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            complete: false,
        };
        publish_private_json(&meta_path, &metadata)?;
        metadata
    };
    let mut file = private_file(&data_path, true)?;
    let length = file.metadata()?.len();
    ensure!(*offset <= length, "image chunk is out of order");
    if *offset < length {
        ensure!(next <= length, "image retry overlaps unwritten bytes");
        let mut previous = vec![0; bytes.len()];
        file.seek(SeekFrom::Start(*offset))?;
        file.read_exact(&mut previous)?;
        ensure!(
            previous == bytes,
            "attachment chunk conflicts with previously uploaded bytes"
        );
    } else {
        ensure!(!metadata.complete, "completed image cannot be modified");
        file.seek(SeekFrom::Start(*offset))?;
        file.write_all(&bytes)?;
        file.sync_data()?;
    }
    if next == *total_bytes && !metadata.complete {
        let mut full = Vec::with_capacity(*total_bytes as usize);
        file.seek(SeekFrom::Start(0))?;
        file.take(MAX_IMAGE_BYTES + 1).read_to_end(&mut full)?;
        if let Err(error) = validate_image(&full, mime_type) {
            fs::remove_file(&data_path)?;
            fs::remove_file(&meta_path)?;
            return Err(error);
        }
        metadata.complete = true;
        publish_private_json(&meta_path, &metadata)?;
    }
    // Reply to replayed earlier chunks as originally: progress is chunk-relative.
    let complete = next == *total_bytes;
    Ok(RemoteResult::ImageUploaded {
        attachment_id: attachment_id.clone(),
        next_offset: next,
        total_bytes: *total_bytes,
        complete,
        prompt_reference: complete.then(|| format!("{MARKER}{})", data_path.display())),
    })
}

pub(super) fn validate_prompt(root: &Path, device: Option<&str>, prompt: &str) -> Result<()> {
    let expanded = rustcode_core::paste::expand(prompt);
    if !expanded.contains(MARKER) {
        return Ok(());
    }
    let device = device.context("image prompts require an authenticated device")?;
    let directory = device_directory(&root.canonicalize()?, device);
    let mut count = 0;
    for suffix in expanded.split(MARKER).skip(1) {
        count += 1;
        ensure!(count <= 8, "a prompt may contain at most 8 images");
        let (path, _) = suffix
            .split_once(')')
            .context("incomplete image reference")?;
        let path = Path::new(path);
        ensure!(
            path.parent() == Some(directory.as_path()),
            "image reference belongs to another session or device"
        );
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("invalid image reference")?;
        ensure!(valid_id(id), "invalid image reference");
        let metadata: Metadata = read_private_json(&path.with_extension("json"), 2048)?;
        ensure!(
            metadata.complete
                && path.extension().and_then(|s| s.to_str())
                    == Some(extension(&metadata.mime_type)?),
            "image upload is not complete"
        );
        let file = private_file(path, false)?;
        ensure!(
            file.metadata()?.len() == metadata.total_bytes,
            "uploaded image changed"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::RemoteOperation;
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(width, height)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }
    fn chunk(bytes: &[u8], offset: usize, length: usize) -> RemoteOperation {
        RemoteOperation::UploadImage {
            attachment_id: "image-1".into(),
            mime_type: "image/png".into(),
            offset: offset as u64,
            total_bytes: bytes.len() as u64,
            data_base64: STANDARD.encode(&bytes[offset..offset + length]),
        }
    }
    fn reference(reply: RemoteResult) -> String {
        let RemoteResult::ImageUploaded {
            prompt_reference: Some(reference),
            complete: true,
            ..
        } = reply
        else {
            panic!("not complete")
        };
        reference
    }
    #[test]
    fn chunks_replay_exact_progress_and_feed_actual_provider_image_parts() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("images");
        let bytes = png(2, 3);
        let first = chunk(&bytes, 0, 20);
        let reply = upload(&root, "device-a", &first).unwrap();
        assert!(matches!(
            reply,
            RemoteResult::ImageUploaded {
                complete: false,
                next_offset: 20,
                ..
            }
        ));
        let last = chunk(&bytes, 20, bytes.len() - 20);
        let final_reply = upload(&root, "device-a", &last).unwrap();
        assert_eq!(upload(&root, "device-a", &first).unwrap(), reply);
        assert_eq!(upload(&root, "device-a", &last).unwrap(), final_reply);
        let marker = reference(final_reply);
        validate_prompt(&root, Some("device-a"), &marker).unwrap();
        assert!(validate_prompt(&root, Some("device-b"), &marker).is_err());
        assert!(validate_prompt(&root, None, &marker).is_err());
        let other = temp.path().join("other-session");
        fs::create_dir(&other).unwrap();
        assert!(validate_prompt(&other, Some("device-a"), &marker).is_err());
        let parts = crate::network::parse_multimodal_content(&format!("Describe this\n{marker}"));
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(
            parts[1]["image_url"]["url"],
            format!("data:image/png;base64,{}", STANDARD.encode(bytes))
        );
    }
    #[test]
    fn reject_conflicting_out_of_order_incomplete_and_arbitrary_references() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("images");
        let bytes = png(1, 1);
        assert!(upload(&root, "device", &chunk(&bytes, 10, 5)).is_err());
        upload(&root, "device", &chunk(&bytes, 0, 20)).unwrap();
        let mut changed = bytes.clone();
        changed[10] ^= 1;
        assert!(upload(&root, "device", &chunk(&changed, 0, 20)).is_err());
        let partial_path =
            device_directory(&root.canonicalize().unwrap(), "device").join("image-1.png");
        assert!(
            validate_prompt(
                &root,
                Some("device"),
                &format!("{MARKER}{})", partial_path.display())
            )
            .is_err()
        );
        assert!(validate_prompt(&root, Some("device"), "![image](file:///etc/passwd)").is_err());
        assert!(!valid_id("../a"));
        assert!(!valid_id("a/b"));
        assert!(!valid_id(""));
    }
    #[test]
    fn reject_malformed_mismatched_and_oversized_images_and_release_partial_budget() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("images");
        let invalid = b"this is not an image";
        assert!(upload(&root, "device", &chunk(invalid, 0, invalid.len())).is_err());
        assert_eq!(reserved_bytes(&root).unwrap(), (0, 0));
        let bytes = png(2, 2);
        let mut request = chunk(&bytes, 0, bytes.len());
        if let RemoteOperation::UploadImage { mime_type, .. } = &mut request {
            *mime_type = "image/jpeg".into();
        }
        assert!(upload(&root, "device", &request).is_err());
        assert!(validate_image(&png(4097, 1), "image/png").is_err());
        assert!(validate_image(&png(1, 4097), "image/png").is_err());
        let mut request = chunk(&bytes, 0, bytes.len());
        if let RemoteOperation::UploadImage { total_bytes, .. } = &mut request {
            *total_bytes = MAX_IMAGE_BYTES + 1;
        }
        assert!(upload(&root, "device", &request).is_err());
    }
    #[test]
    fn partial_reservations_share_a_bounded_session_budget_across_devices() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("images");
        let bytes = png(1, 1);
        for index in 0..8 {
            let mut request = chunk(&bytes, 0, 8);
            if let RemoteOperation::UploadImage { total_bytes, .. } = &mut request {
                *total_bytes = MAX_IMAGE_BYTES;
            }
            upload(&root, &format!("device-{index}"), &request).unwrap();
        }
        assert_eq!(reserved_bytes(&root).unwrap(), (MAX_SESSION_BYTES, 8));
        let request = chunk(&bytes, 0, bytes.len());
        assert!(
            upload(&root, "ninth-device", &request)
                .unwrap_err()
                .to_string()
                .contains("storage is full")
        );
    }

    #[test]
    fn reject_oversized_base64_chunks_and_unsafe_attachment_ids_before_storage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("images");
        let bytes = png(1, 1);
        let mut request = chunk(&bytes, 0, bytes.len());
        if let RemoteOperation::UploadImage { data_base64, .. } = &mut request {
            *data_base64 = "a".repeat(MAX_CHUNK_BYTES * 2);
        }
        assert!(upload(&root, "device", &request).is_err());
        let mut request = chunk(&bytes, 0, bytes.len());
        if let RemoteOperation::UploadImage { attachment_id, .. } = &mut request {
            *attachment_id = "../../escape".into();
        }
        assert!(upload(&root, "device", &request).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn expired_partial_uploads_are_removed_but_complete_images_remain_private() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("images");
        let bytes = png(2, 2);
        upload(&root, "expired", &chunk(&bytes, 0, 20)).unwrap();
        let meta = device_directory(&root.canonicalize().unwrap(), "expired").join("image-1.json");
        let mut metadata: Metadata = read_private_json(&meta, 2048).unwrap();
        metadata.created = 0;
        publish_private_json(&meta, &metadata).unwrap();
        let marker = reference(upload(&root, "complete", &chunk(&bytes, 0, bytes.len())).unwrap());
        assert!(!meta.exists());
        let path = marker
            .strip_prefix(MARKER)
            .unwrap()
            .strip_suffix(')')
            .unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(reserved_bytes(&root).unwrap(), (bytes.len() as u64, 1));
        validate_prompt(&root, Some("complete"), &marker).unwrap();
    }
}
