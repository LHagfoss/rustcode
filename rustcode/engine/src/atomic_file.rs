use std::path::Path;

pub(crate) use rustcode_session::atomic_file::{replace_file, write_atomic};

/// [`write_atomic`] with an error that names the file, for callers that
/// report errors as text.
pub(crate) fn write_described(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic(path, bytes)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}
