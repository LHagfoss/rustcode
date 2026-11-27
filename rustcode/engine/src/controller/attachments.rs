//! Persist native frontend images where the existing multimodal prompt loader
//! can read them again when a saved session is resumed, and hand them back to
//! a client that shows the thread later: this computer's app, or one on a
//! paired computer, which reads and sends them in chunks small enough for the
//! remote connection's frames (`docs/remote.md` › Images in remote threads).

use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use agent_client_protocol::schema::v1::{ContentBlock, ImageContent};
use base64::{Engine as _, engine::general_purpose::STANDARD};

const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(0);
/// Raw bytes per chunk an image is read or uploaded in, just under 512 KiB: as
/// base64 inside a JSON-RPC message it stays well under the remote
/// connection's 1 MiB frame, and a multiple of 3 so the base64 of consecutive
/// chunks joins up without padding in between.
pub const IMAGE_CHUNK_BYTES: usize = 3 * 174_762;
/// The prefix the transcript's image markers start with.
pub const IMAGE_MARKER: &str = "![image](file://";

/// Save an image from a native frontend and return the absolute path to use in
/// a `![image](file://...)` user prompt. The prompt loader supports these four
/// encoded formats without conversion.
pub fn save_image_attachment(bytes: &[u8], extension: &str) -> io::Result<PathBuf> {
    save_image_in(&attachments_dir()?, bytes, extension)
}

fn config_dir() -> io::Result<PathBuf> {
    crate::config::get_config_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "RustCode config directory unavailable",
        )
    })
}

/// The folder attached images are kept in.
pub fn attachments_dir() -> io::Result<PathBuf> {
    Ok(config_dir()?.join("attachments"))
}

/// The file extension an image MIME type is stored under.
pub fn image_extension(mime: &str) -> Option<&'static str> {
    match mime.to_ascii_lowercase().as_str() {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

/// The MIME type of a stored image, from its extension.
pub fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The image format the bytes start with, as a MIME type.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// The transcript marker for an image kept at `path`.
pub fn image_marker(path: &Path) -> String {
    format!("{IMAGE_MARKER}{})", path.display())
}

/// A stored image a client may read back, from a marker's path or a
/// `file://` URI: an image file in `attachments/`, or one a paired device
/// uploaded into a thread's `remote-images/`. Anything else on this computer
/// is never served, whoever asks.
pub fn stored_image(path_or_uri: &str) -> Option<PathBuf> {
    stored_image_in(&config_dir().ok()?, path_or_uri)
}

fn stored_image_in(config_dir: &Path, path_or_uri: &str) -> Option<PathBuf> {
    let raw = path_or_uri.strip_prefix("file://").unwrap_or(path_or_uri);
    let path = Path::new(raw);
    if !path.is_absolute() {
        return None;
    }
    image_mime(path)?;
    let canonical = fs::canonicalize(path).ok()?;
    if !canonical.is_file() {
        return None;
    }
    let root = fs::canonicalize(config_dir).ok()?;
    let attached = canonical.starts_with(root.join("attachments"));
    let uploaded_by_device = canonical.starts_with(&root)
        && canonical
            .components()
            .any(|part| part.as_os_str() == "remote-images");
    (attached || uploaded_by_device).then_some(canonical)
}

/// One chunk of a stored image, as `_rustcode/attachments/read` answers it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageChunk {
    pub mime_type: String,
    /// The whole file's size in bytes.
    pub size: u64,
    pub offset: u64,
    /// Base64 of the bytes from `offset`, at most [`IMAGE_CHUNK_BYTES`].
    pub data: String,
    pub done: bool,
}

/// Read the chunk of the stored image `path_or_uri` that starts at `offset`.
pub fn read_image_chunk(path_or_uri: &str, offset: u64) -> Result<ImageChunk, String> {
    read_image_chunk_in(
        &config_dir().map_err(|error| error.to_string())?,
        path_or_uri,
        offset,
    )
}

fn read_image_chunk_in(
    config_dir: &Path,
    path_or_uri: &str,
    offset: u64,
) -> Result<ImageChunk, String> {
    let path = stored_image_in(config_dir, path_or_uri)
        .ok_or_else(|| "that image is not kept on this computer".to_owned())?;
    read_chunk(&path, offset, false)
}

/// One chunk of a picture a thread's reply names (`![alt](path)`): a stored
/// image as [`read_image_chunk`] serves it, or else an image file inside the
/// thread's own folders (its working directory, added directories and the
/// folder its harness keeps the session's output in) or this
/// computer's scratch folders, where an agent saves its screenshots. A paired
/// computer's app cannot read this disk, so it asks for them here.
pub fn read_thread_image_chunk(
    session_id: &str,
    path_or_uri: &str,
    offset: u64,
) -> Result<ImageChunk, String> {
    let config_dir = config_dir().map_err(|error| error.to_string())?;
    if let Some(path) = stored_image_in(&config_dir, path_or_uri) {
        return read_chunk(&path, offset, false);
    }
    if !rustcode_session::valid_session_id(session_id) {
        return Err("no such thread".to_owned());
    }
    let workspace = crate::sessions::load_session_workspace(session_id);
    let cwd = workspace
        .as_ref()
        .map(|workspace| workspace.cwd.clone())
        .or_else(|| crate::checkpoints::thread_cwd(session_id));
    let mut roots: Vec<PathBuf> = cwd.into_iter().collect();
    if let Some(workspace) = workspace {
        roots.extend(workspace.additional_directories);
    }
    roots.extend(crate::sessions::get_active_session_dir(session_id));
    let home = crate::home::home_dir();
    // Where the thread's harness saves its own output (Antigravity's session folder).
    if let (Some(record), Some(home)) = (crate::harness::store::load(session_id), &home) {
        roots.extend(record.native_session_id.and_then(|native| {
            crate::harness::native_artifact_dir(&record.harness, &native, home)
        }));
    }
    let scratch = scratch_roots();
    let path = match thread_image_in(path_or_uri, &roots, &scratch, home.as_deref()) {
        Err(error) if error == OUTSIDE_THREAD_FOLDERS => {
            let checkouts = roots
                .first()
                .map(|cwd| repository_checkouts(cwd))
                .unwrap_or_default();
            if checkouts.is_empty() {
                return Err(error);
            }
            roots.extend(checkouts);
            thread_image_in(path_or_uri, &roots, &scratch, home.as_deref())?
        }
        other => other?,
    };
    read_chunk(&path, offset, true)
}

const OUTSIDE_THREAD_FOLDERS: &str =
    "outside the thread's folders, its repository's worktrees and the temporary folders";

/// Every checkout of `dir`'s repository: the main one and its worktrees.
fn repository_checkouts(dir: &Path) -> Vec<PathBuf> {
    crate::worktrees::git(dir, &["worktree", "list", "--porcelain"])
        .map(|listed| checkouts_in_porcelain(&listed))
        .unwrap_or_default()
}

fn checkouts_in_porcelain(porcelain: &str) -> Vec<PathBuf> {
    porcelain
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .collect()
}

/// The OS scratch folders a thread's pictures may also be in.
fn scratch_roots() -> Vec<PathBuf> {
    let mut roots = vec![std::env::temp_dir()];
    if cfg!(unix) {
        roots.extend([PathBuf::from("/tmp"), PathBuf::from("/private/tmp")]);
    }
    if cfg!(target_os = "macos") {
        roots.extend([
            PathBuf::from("/var/folders"),
            PathBuf::from("/private/var/folders"),
        ]);
    }
    roots
}

/// A reply's picture path resolved (a relative one against the thread's first
/// folder, symlinks followed) and checked to be an image file inside one of
/// `roots` or `scratch`.
fn thread_image_in(
    path_or_uri: &str,
    roots: &[PathBuf],
    scratch: &[PathBuf],
    home: Option<&Path>,
) -> Result<PathBuf, String> {
    let raw = path_or_uri.strip_prefix("file://").unwrap_or(path_or_uri);
    let raw = crate::home::expand_home(raw, home);
    let joined = if raw.is_absolute() {
        raw
    } else {
        roots
            .first()
            .ok_or_else(|| "this thread has no folder".to_owned())?
            .join(raw)
    };
    image_mime(&joined).ok_or_else(|| "not a picture".to_owned())?;
    let real = fs::canonicalize(&joined).map_err(|_| "No such picture.".to_owned())?;
    if !real.is_file() {
        return Err("not a file".to_owned());
    }
    let inside = roots
        .iter()
        .chain(scratch)
        .filter(|root| root.is_absolute())
        .filter_map(|root| fs::canonicalize(root).ok())
        .any(|root| real.starts_with(root));
    if !inside {
        return Err(OUTSIDE_THREAD_FOLDERS.to_owned());
    }
    Ok(real)
}

/// `sniffed`: the first chunk must start the way the image its name says does,
/// so a project file renamed to `.png` is not served.
fn read_chunk(path: &Path, offset: u64, sniffed: bool) -> Result<ImageChunk, String> {
    let mime = image_mime(path).unwrap_or("application/octet-stream");
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size > MAX_IMAGE_BYTES as u64 {
        return Err("the image is larger than 20 MiB".to_owned());
    }
    if offset > size {
        return Err(format!("offset {offset} is past the image's {size} bytes"));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::with_capacity(IMAGE_CHUNK_BYTES.min((size - offset) as usize));
    file.take(IMAGE_CHUNK_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if sniffed && offset == 0 && sniff(&bytes) != Some(mime) {
        return Err("the file's contents are not the picture its name says".to_owned());
    }
    let next = offset + bytes.len() as u64;
    Ok(ImageChunk {
        mime_type: mime.to_owned(),
        size,
        offset,
        data: STANDARD.encode(&bytes),
        done: next >= size,
    })
}

/// What `_rustcode/attachments/upload` answers: the upload to continue, and
/// once the last chunk is in, where the image is kept.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadAnswer {
    pub upload_id: String,
    pub received: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
}

/// One chunk of an image a client sends ahead of the prompt that carries it
/// (`_rustcode/attachments/upload`). The first chunk names no `upload_id` and
/// starts one; every chunk continues where the file ends; the one marked
/// `done` checks the bytes are the image `mime` says and keeps it.
pub fn upload_image_chunk(
    upload_id: Option<&str>,
    mime: &str,
    offset: u64,
    data: &str,
    done: bool,
) -> Result<UploadAnswer, String> {
    let directory = attachments_dir().map_err(|error| error.to_string())?;
    upload_image_chunk_in(&directory, upload_id, mime, offset, data, done)
}

fn upload_image_chunk_in(
    directory: &Path,
    upload_id: Option<&str>,
    mime: &str,
    offset: u64,
    data: &str,
    done: bool,
) -> Result<UploadAnswer, String> {
    let extension = image_extension(mime)
        .ok_or_else(|| format!("unsupported image type {mime}; use PNG, JPEG, GIF or WebP"))?;
    if data.len() > IMAGE_CHUNK_BYTES.div_ceil(3) * 4 + 4 {
        return Err("an upload chunk is larger than 512 KiB".to_owned());
    }
    let bytes = STANDARD
        .decode(data.trim())
        .map_err(|error| format!("image data is not valid base64: {error}"))?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let (id, mut file) = match upload_id {
        None => {
            if offset != 0 {
                return Err("a new upload starts at offset 0".to_owned());
            }
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos();
            let id = format!("{now}_{}", NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed));
            let file = OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(directory.join(format!("upload_{id}.part")))
                .map_err(|error| error.to_string())?;
            (id, file)
        }
        Some(id) => {
            if id.is_empty()
                || id.len() > 64
                || !id.bytes().all(|c| c.is_ascii_digit() || c == b'_')
            {
                return Err("unknown upload".to_owned());
            }
            let file = OpenOptions::new()
                .append(true)
                .open(directory.join(format!("upload_{id}.part")))
                .map_err(|_| "unknown upload; start it again".to_owned())?;
            (id.to_owned(), file)
        }
    };
    let have = file.metadata().map_err(|error| error.to_string())?.len();
    if have != offset {
        return Err(format!(
            "the upload has {have} bytes, not {offset}; send the chunk that starts there"
        ));
    }
    if have + bytes.len() as u64 > MAX_IMAGE_BYTES as u64 {
        let _ = fs::remove_file(directory.join(format!("upload_{id}.part")));
        return Err("images must be 20 MiB or smaller".to_owned());
    }
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    let received = have + bytes.len() as u64;
    if !done {
        return Ok(UploadAnswer {
            upload_id: id,
            received,
            path: None,
            uri: None,
        });
    }
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    let part = directory.join(format!("upload_{id}.part"));
    let mut head = [0u8; 16];
    let read = fs::File::open(&part)
        .and_then(|mut file| file.read(&mut head))
        .map_err(|error| error.to_string())?;
    if received == 0 || sniff(&head[..read]) != Some(normal_mime(mime)) {
        let _ = fs::remove_file(&part);
        return Err(format!("the uploaded bytes are not a {mime} image"));
    }
    let kept = directory.join(format!("upload_{id}.{extension}"));
    crate::atomic_file::replace_file(&part, &kept).map_err(|error| error.to_string())?;
    let kept = fs::canonicalize(&kept).unwrap_or(kept);
    Ok(UploadAnswer {
        upload_id: id,
        received,
        uri: Some(format!("file://{}", kept.display())),
        path: Some(kept.display().to_string()),
    })
}

fn normal_mime(mime: &str) -> &'static str {
    match image_extension(mime) {
        Some("png") => "image/png",
        Some("jpg") => "image/jpeg",
        Some("gif") => "image/gif",
        _ => "image/webp",
    }
}

/// Make every image in a prompt a stored one, before the turn sees it: an
/// image sent by reference (no data, the `file://` URI an upload answered)
/// gets its bytes back for the harness, and an image sent inline is saved, so
/// the thread can show it again after a restart or on another computer. The
/// image's `uri` names the stored file afterwards ([`stored_image`]).
pub fn materialize_images(prompt: &mut [ContentBlock]) -> Result<(), String> {
    for block in prompt {
        if let ContentBlock::Image(image) = block {
            materialize_image(image, save_image_attachment)?;
        }
    }
    Ok(())
}

fn materialize_image(
    image: &mut ImageContent,
    save: impl FnOnce(&[u8], &str) -> io::Result<PathBuf>,
) -> Result<(), String> {
    let stored = image.uri.as_deref().and_then(stored_image);
    if image.data.trim().is_empty() {
        let path = stored.ok_or_else(|| {
            "an image came without its data, and its file is not kept on this computer".to_owned()
        })?;
        let bytes =
            fs::read(&path).map_err(|error| format!("could not read the image: {error}"))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("images must be 20 MiB or smaller".to_owned());
        }
        image.data = STANDARD.encode(&bytes);
        image.uri = Some(format!("file://{}", path.display()));
        return Ok(());
    }
    if stored.is_some() {
        return Ok(());
    }
    // An image this computer cannot keep still goes to the turn as it came;
    // the prompt's own checks say what is wrong with it.
    let Some(extension) = image_extension(&image.mime_type) else {
        return Ok(());
    };
    let Ok(bytes) = STANDARD.decode(image.data.trim()) else {
        return Ok(());
    };
    if let Ok(path) = save(&bytes, extension) {
        image.uri = Some(format!("file://{}", path.display()));
    }
    Ok(())
}

/// The marker a recorded message keeps for an image whose file is stored.
pub fn stored_image_marker(image: &ImageContent) -> Option<String> {
    image
        .uri
        .as_deref()
        .and_then(stored_image)
        .map(|path| image_marker(&path))
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
        match crate::atomic_file::write_atomic_new(&path, bytes) {
            Ok(()) => return Ok(path),
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

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n-rest-of-a-png";

    #[test]
    fn serves_a_thread_picture_only_inside_its_folders() {
        let project = tempfile::tempdir().expect("tempdir");
        let scratch = tempfile::tempdir().expect("tempdir");
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let roots = [project.path().to_path_buf()];
        let scratch_roots = [scratch.path().to_path_buf()];
        fs::create_dir_all(project.path().join("shots")).unwrap();
        fs::write(project.path().join("shots/a.png"), PNG).unwrap();
        fs::write(scratch.path().join("b.png"), PNG).unwrap();
        fs::write(elsewhere.path().join("c.png"), PNG).unwrap();
        fs::write(project.path().join("notes.png"), b"not a picture").unwrap();
        fs::write(project.path().join("notes.txt"), b"text").unwrap();
        let resolve = |path: &str| thread_image_in(path, &roots, &scratch_roots, None);

        // Relative to the thread's folder, absolute, and a file:// URI.
        let relative = resolve("shots/a.png").unwrap();
        assert_eq!(
            resolve(&project.path().join("shots/a.png").display().to_string()).unwrap(),
            relative
        );
        let scratch_file = scratch.path().join("b.png");
        assert!(resolve(&format!("file://{}", scratch_file.display())).is_ok());
        // Outside every folder, not a picture, or missing.
        assert!(resolve(&elsewhere.path().join("c.png").display().to_string()).is_err());
        assert!(resolve("../escape.png").is_err());
        assert!(resolve("notes.txt").is_err());
        assert!(resolve("missing.png").is_err());

        assert!(read_chunk(&relative, 0, true).unwrap().done);
        let renamed = resolve("notes.png").unwrap();
        assert!(read_chunk(&renamed, 0, true).is_err());
    }

    #[test]
    fn lists_every_checkout_of_a_repository() {
        let porcelain = "worktree /home/ada/code/rustcode\nHEAD 1a2b\nbranch refs/heads/main\n\n\
                         worktree /home/ada/code/worktrees/rustcode-streak\nHEAD 3c4d\n\
                         branch refs/heads/feat/streak\n\nworktree /tmp/detached\nHEAD 5e6f\ndetached\n";
        assert_eq!(
            checkouts_in_porcelain(porcelain),
            [
                PathBuf::from("/home/ada/code/rustcode"),
                PathBuf::from("/home/ada/code/worktrees/rustcode-streak"),
                PathBuf::from("/tmp/detached"),
            ]
        );
        assert!(checkouts_in_porcelain("").is_empty());
    }

    #[test]
    fn serves_only_images_kept_in_the_attachment_stores() {
        let config = tempfile::tempdir().expect("tempdir");
        let kept = save_image_in(&config.path().join("attachments"), PNG, "png").unwrap();
        assert_eq!(
            stored_image_in(config.path(), &format!("file://{}", kept.display())),
            Some(fs::canonicalize(&kept).unwrap())
        );
        // A device upload in a thread's own folder.
        let device = config.path().join("threads/x/artifacts/remote-images/dev");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("a.jpg"), b"jpg").unwrap();
        assert!(stored_image_in(config.path(), device.join("a.jpg").to_str().unwrap()).is_some());
        // Not an image, outside the stores, relative, or climbing out of them.
        fs::write(config.path().join("attachments/notes.txt"), b"x").unwrap();
        fs::write(config.path().join("secret.png"), PNG).unwrap();
        for refused in [
            config
                .path()
                .join("attachments/notes.txt")
                .display()
                .to_string(),
            config.path().join("secret.png").display().to_string(),
            config
                .path()
                .join("attachments/../secret.png")
                .display()
                .to_string(),
            "attachments/secret.png".to_owned(),
        ] {
            assert_eq!(stored_image_in(config.path(), &refused), None, "{refused}");
        }
    }

    #[test]
    fn reads_an_image_back_in_chunks() {
        let config = tempfile::tempdir().expect("tempdir");
        let mut bytes = PNG.to_vec();
        bytes.resize(IMAGE_CHUNK_BYTES + 10, 7);
        let kept = save_image_in(&config.path().join("attachments"), &bytes, "png").unwrap();
        let path = kept.display().to_string();
        let first = read_image_chunk_in(config.path(), &path, 0).unwrap();
        assert_eq!(first.mime_type, "image/png");
        assert_eq!(first.size, bytes.len() as u64);
        assert!(!first.done);
        let second = read_image_chunk_in(config.path(), &path, IMAGE_CHUNK_BYTES as u64).unwrap();
        assert!(second.done);
        let mut joined = STANDARD.decode(first.data).unwrap();
        joined.extend(STANDARD.decode(second.data).unwrap());
        assert_eq!(joined, bytes);
        assert!(read_image_chunk_in(config.path(), "/etc/hosts", 0).is_err());
    }

    #[test]
    fn uploads_an_image_in_chunks_and_keeps_it() {
        let directory = tempfile::tempdir().expect("tempdir");
        let (head, tail) = PNG.split_at(5);
        let started = upload_image_chunk_in(
            directory.path(),
            None,
            "image/png",
            0,
            &STANDARD.encode(head),
            false,
        )
        .unwrap();
        assert_eq!(started.received, 5);
        assert!(started.path.is_none());
        // A chunk that does not continue where the file ends is refused.
        assert!(
            upload_image_chunk_in(
                directory.path(),
                Some(&started.upload_id),
                "image/png",
                2,
                &STANDARD.encode(tail),
                true
            )
            .is_err()
        );
        let done = upload_image_chunk_in(
            directory.path(),
            Some(&started.upload_id),
            "image/png",
            5,
            &STANDARD.encode(tail),
            true,
        )
        .unwrap();
        let path = PathBuf::from(done.path.unwrap());
        assert_eq!(fs::read(&path).unwrap(), PNG);
        assert_eq!(done.uri.unwrap(), format!("file://{}", path.display()));
        // An id that could name another file is never opened.
        assert!(
            upload_image_chunk_in(directory.path(), Some("../x"), "image/png", 0, "", true)
                .is_err()
        );
    }

    #[test]
    fn an_upload_that_is_not_the_named_image_is_dropped() {
        let directory = tempfile::tempdir().expect("tempdir");
        let error = upload_image_chunk_in(
            directory.path(),
            None,
            "image/png",
            0,
            &STANDARD.encode(b"plain text"),
            true,
        )
        .unwrap_err();
        assert!(error.contains("not a image/png image"), "{error}");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_inline_image_is_saved_and_named_by_its_uri() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut image = ImageContent::new(STANDARD.encode(PNG), "image/png");
        materialize_image(&mut image, |bytes, extension| {
            save_image_in(directory.path(), bytes, extension)
        })
        .unwrap();
        let uri = image.uri.clone().expect("a uri");
        let path = uri.strip_prefix("file://").unwrap();
        assert_eq!(fs::read(path).unwrap(), PNG);
        assert_eq!(image.data, STANDARD.encode(PNG));
    }

    #[test]
    fn an_image_by_reference_must_be_a_kept_one() {
        let mut image = ImageContent::new("", "image/png").uri("file:///etc/passwd.png".to_owned());
        assert!(materialize_image(&mut image, |_, _| unreachable!()).is_err());
        assert!(image.data.is_empty());
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
