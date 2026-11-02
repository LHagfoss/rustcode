//! The new-thread banner picture, chosen in Settings ▸ Appearance. A copy is
//! kept in the app's data folder, so moving or deleting the original keeps
//! the banner; the page holds only the copy's file name. Names are checked
//! against the one shape this module writes, so a page cannot read or delete
//! anything else.

use crate::native::{image_mime, ImageFile};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const PREFIX: &str = "new-thread-banner-";
const MAX_BYTES: u64 = 40 * 1024 * 1024;

fn folder(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

/// A name this module wrote: `new-thread-banner-<8 hex>.<image extension>`.
pub fn valid_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix(PREFIX) else {
        return false;
    };
    let Some((id, ext)) = rest.split_once('.') else {
        return false;
    };
    id.len() == 8
        && id.bytes().all(|b| b.is_ascii_hexdigit())
        && image_mime(Path::new(name)).is_some()
        && !ext.contains('.')
}

fn new_name(source: &Path) -> Option<String> {
    let ext = source.extension()?.to_str()?.to_ascii_lowercase();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let name = format!("{PREFIX}{:08x}.{ext}", (nanos as u32) ^ std::process::id());
    valid_name(&name).then_some(name)
}

/// Copies the picture at `source` into the data folder and returns the
/// copy's name. The previous banner, `replacing`, is removed after.
#[tauri::command(async)]
pub fn banner_save(
    app: AppHandle,
    source: String,
    replacing: Option<String>,
) -> Result<String, String> {
    let source = PathBuf::from(source);
    if !source.is_absolute() || !source.is_file() {
        return Err("Not a picture file.".into());
    }
    let name = new_name(&source).ok_or("Choose a PNG, JPEG, GIF or WebP picture.")?;
    let meta = std::fs::metadata(&source).map_err(|e| e.to_string())?;
    if meta.len() > MAX_BYTES {
        return Err("The picture must be 40 MB or smaller.".into());
    }
    let dir = folder(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::fs::copy(&source, dir.join(&name)).map_err(|e| e.to_string())?;
    if let Some(old) = replacing.filter(|old| valid_name(old) && *old != name) {
        let _ = std::fs::remove_file(dir.join(old));
    }
    Ok(name)
}

/// The kept picture `name`, as base64 for a `data:` URL.
#[tauri::command(async)]
pub fn banner_read(app: AppHandle, name: String) -> Result<ImageFile, String> {
    use base64::Engine as _;
    if !valid_name(&name) {
        return Err("Not a banner.".into());
    }
    let path = folder(&app)?.join(&name);
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(ImageFile {
        mime_type: image_mime(&path).unwrap_or("image/png").into(),
        name,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

#[tauri::command(async)]
pub fn banner_remove(app: AppHandle, name: String) -> Result<(), String> {
    if !valid_name(&name) {
        return Err("Not a banner.".into());
    }
    let _ = std::fs::remove_file(folder(&app)?.join(name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_its_own_names() {
        assert!(valid_name("new-thread-banner-0a1b2c3d.png"));
        assert!(valid_name("new-thread-banner-0a1b2c3d.JPEG"));
        assert!(!valid_name("new-thread-banner-0a1b2c3d.txt"));
        assert!(!valid_name("new-thread-banner-../../x.png"));
        assert!(!valid_name("new-thread-banner-0a1b2c3d.png.png"));
        assert!(!valid_name("other-0a1b2c3d.png"));
        assert!(!valid_name("new-thread-banner-0a1b2c.png"));
    }

    #[test]
    fn makes_a_valid_name_for_an_image() {
        let name = new_name(Path::new("/tmp/Wide Photo.JPG")).unwrap();
        assert!(valid_name(&name), "{name}");
        assert!(new_name(Path::new("/tmp/notes.txt")).is_none());
    }
}
