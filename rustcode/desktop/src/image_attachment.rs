//! Image clipboard support for the native chat composer.

use std::{fs, path::PathBuf};

use gpui_kit::{ClipboardEntry, ClipboardItem, ImageFormat};
use rustcode::controller::save_image_attachment;

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const IMAGE_MARKER: &str = "![image](file://";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageAttachment {
    pub path: PathBuf,
}

pub fn contains_images(item: &ClipboardItem) -> bool {
    item.entries().iter().any(|entry| match entry {
        ClipboardEntry::Image(_) => true,
        ClipboardEntry::ExternalPaths(paths) => paths.0.iter().any(|path| is_supported_path(path)),
        _ => false,
    })
}

impl ImageAttachment {
    fn marker(&self) -> String {
        format!("{IMAGE_MARKER}{})", self.path.display())
    }
}

/// Return `None` for ordinary text, so Textarea handles it normally. Clipboard
/// images and copied image files are copied into RustCode's attachment store.
pub fn paste_images(item: &ClipboardItem) -> Result<Option<Vec<ImageAttachment>>, String> {
    let direct_images = item
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !direct_images.is_empty() {
        let mut attachments = Vec::with_capacity(direct_images.len());
        for image in direct_images {
            let extension = supported_extension(image.format).ok_or_else(|| {
                format!(
                    "Cannot paste {} images. Use PNG, JPEG, GIF, or WebP.",
                    image.format.extension().to_uppercase()
                )
            })?;
            let path = save_image_attachment(&image.bytes, extension)
                .map_err(|error| format!("Could not save pasted image: {error}"))?;
            attachments.push(ImageAttachment { path });
        }
        return Ok(Some(attachments));
    }

    let image_paths = item
        .entries()
        .iter()
        .filter_map(|entry| match entry {
            ClipboardEntry::ExternalPaths(paths) => Some(paths.0.as_slice()),
            _ => None,
        })
        .flatten()
        .filter_map(|path| {
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
                .then_some((path, extension))
        })
        .collect::<Vec<_>>();
    if image_paths.is_empty() {
        return Ok(None);
    }
    let mut attachments = Vec::with_capacity(image_paths.len());
    for (source, extension) in image_paths {
        let size = fs::metadata(source)
            .map_err(|error| format!("Could not read pasted image: {error}"))?
            .len();
        if size > MAX_IMAGE_BYTES {
            return Err("Pasted image is larger than 20 MiB".to_owned());
        }
        let bytes =
            fs::read(source).map_err(|error| format!("Could not read pasted image: {error}"))?;
        let path = save_image_attachment(&bytes, &extension)
            .map_err(|error| format!("Could not save pasted image: {error}"))?;
        attachments.push(ImageAttachment { path });
    }
    Ok(Some(attachments))
}

fn supported_extension(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("png"),
        ImageFormat::Jpeg => Some("jpg"),
        ImageFormat::Gif => Some("gif"),
        ImageFormat::Webp => Some("webp"),
        _ => None,
    }
}

/// Keep the existing multimodal marker protocol intact. Image-only prompts
/// remain valid because the returned string contains an image marker.
pub fn prompt_with_images(text: &str, attachments: &[ImageAttachment]) -> String {
    if attachments.is_empty() {
        return text.to_owned();
    }
    let markers = attachments
        .iter()
        .map(ImageAttachment::marker)
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        markers
    } else {
        format!("{}\n{markers}", text.trim_end())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserPart {
    Text(String),
    Image(PathBuf),
}

/// Split stored user text into readable text and local images. Unclosed or
/// invalid markers stay visible as text, preserving the original transcript.
pub fn user_parts(content: &str) -> Vec<UserPart> {
    let mut remaining = content;
    let mut parts = Vec::new();
    while let Some(start) = remaining.find(IMAGE_MARKER) {
        let after_prefix = &remaining[start + IMAGE_MARKER.len()..];
        let Some(end) = after_prefix.find(')') else {
            break;
        };
        let path = PathBuf::from(&after_prefix[..end]);
        if !path.is_absolute() || !is_supported_path(&path) {
            let plain_end = start + IMAGE_MARKER.len() + end + 1;
            push_text(&mut parts, &remaining[..plain_end]);
            remaining = &remaining[plain_end..];
            continue;
        }
        push_text(&mut parts, &remaining[..start]);
        parts.push(UserPart::Image(path));
        remaining = &after_prefix[end + 1..];
    }
    push_text(&mut parts, remaining);
    parts
}

fn is_supported_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp"
            )
        })
}

fn push_text(parts: &mut Vec<UserPart>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(UserPart::Text(previous)) = parts.last_mut() {
        previous.push_str(text);
    } else {
        parts.push(UserPart::Text(text.to_owned()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_only_prompt_contains_supported_multimodal_marker() {
        let attachments = [ImageAttachment {
            path: PathBuf::from("/tmp/screenshot.png"),
        }];
        assert_eq!(
            prompt_with_images("", &attachments),
            "![image](file:///tmp/screenshot.png)"
        );
        assert_eq!(
            prompt_with_images("Explain this", &attachments),
            "Explain this\n![image](file:///tmp/screenshot.png)"
        );
    }

    #[test]
    fn splits_multiple_images_and_preserves_unicode_text() {
        assert_eq!(
            user_parts("Se! ![image](file:///tmp/a.png) og ![image](file:///tmp/b.jpg)"),
            vec![
                UserPart::Text("Se! ".to_owned()),
                UserPart::Image(PathBuf::from("/tmp/a.png")),
                UserPart::Text(" og ".to_owned()),
                UserPart::Image(PathBuf::from("/tmp/b.jpg")),
            ]
        );
    }

    #[test]
    fn leaves_invalid_and_unclosed_markers_as_text() {
        for content in [
            "![image](file://relative.png)",
            "![image](file:///tmp/script.rs)",
            "![image](file:///tmp/unclosed.png",
        ] {
            assert_eq!(
                user_parts(content),
                vec![UserPart::Text(content.to_owned())]
            );
        }
    }
}
