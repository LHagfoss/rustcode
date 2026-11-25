use std::path::Path;

pub(crate) use rustcode_session::atomic_file::{replace_file, write_atomic};

/// [`write_atomic`] into a folder created when missing, with an error that
/// names the file, for callers that report errors as text.
pub(crate) fn write_described(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    write_atomic(path, bytes)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}
