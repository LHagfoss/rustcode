//! Export a saved session as one zip archive, so it can be sent to another
//! machine and analyzed there (`rustcode sessions export <id>`, `/export`).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Directories of a session that are never exported: `sandbox/` is the
/// session's scratch workspace, not evidence about the session.
const SKIPPED_DIRECTORIES: &[&str] = &["sandbox"];

/// Write `<destination>/rustcode-session-<id>.zip` from the session's saved
/// directory and return its path. `destination` defaults to the user's
/// Downloads folder. An earlier export of the same session is replaced.
pub fn export_session(session_id: &str, destination: Option<&Path>) -> Result<PathBuf, String> {
    let session_id = session_id.trim();
    if session_id.is_empty() || session_id.contains(['/', '\\']) || session_id.starts_with('.') {
        return Err(format!("`{session_id}` is not a session id."));
    }
    let source = super::get_active_session_dir(session_id)
        .ok_or_else(|| "Configuration directory is unavailable.".to_owned())?;
    if !source.is_dir() {
        return Err(format!("No saved session with id `{session_id}`."));
    }
    let destination = match destination {
        Some(path) => path.to_path_buf(),
        None => downloads_dir().ok_or_else(|| "Home directory is unavailable.".to_owned())?,
    };
    std::fs::create_dir_all(&destination)
        .map_err(|error| format!("Could not create {}: {error}", destination.display()))?;
    let archive_path = destination.join(format!("rustcode-session-{session_id}.zip"));
    write_archive(session_id, &source, &archive_path)?;
    Ok(archive_path)
}

/// The user's Downloads folder: `XDG_DOWNLOAD_DIR` (from the environment or
/// `user-dirs.dirs`) on Linux, otherwise `~/Downloads`.
pub fn downloads_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?);
    let xdg = std::env::var("XDG_DOWNLOAD_DIR").ok().or_else(|| {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let dirs = std::fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
        user_dirs_download_entry(&dirs)
    });
    Some(match xdg {
        Some(path) => PathBuf::from(path.replace("$HOME", &home.to_string_lossy())),
        None => home.join("Downloads"),
    })
}

fn user_dirs_download_entry(contents: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let value = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=")?;
        let value = value.trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn write_archive(session_id: &str, source: &Path, archive_path: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_files(source, source, &mut files)
        .map_err(|error| format!("Could not read {}: {error}", source.display()))?;
    files.sort();

    // Write next to the target and rename, so a failed export never leaves a
    // truncated archive under the final name.
    let partial = archive_path.with_extension("zip.partial");
    let result = (|| -> Result<(), String> {
        let file = std::fs::File::create(&partial)
            .map_err(|error| format!("Could not create {}: {error}", partial.display()))?;
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        let root = format!("rustcode-session-{session_id}");
        let zip_error = |error: zip::result::ZipError| format!("Could not write archive: {error}");
        let io_error = |error: std::io::Error| format!("Could not write archive: {error}");

        zip.start_file(format!("{root}/EXPORT.md"), options)
            .map_err(zip_error)?;
        zip.write_all(export_readme(session_id, &files).as_bytes())
            .map_err(io_error)?;
        for relative in &files {
            let name = relative
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            zip.start_file(format!("{root}/{name}"), options)
                .map_err(zip_error)?;
            let mut input = std::fs::File::open(source.join(relative))
                .map_err(|error| format!("Could not read {}: {error}", relative.display()))?;
            std::io::copy(&mut input, &mut zip).map_err(io_error)?;
        }
        zip.finish().map_err(zip_error)?;
        Ok(())
    })();
    match result {
        Ok(()) => std::fs::rename(&partial, archive_path).map_err(|error| {
            let _ = std::fs::remove_file(&partial);
            format!("Could not write {}: {error}", archive_path.display())
        }),
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error)
        }
    }
}

/// Regular files under `directory`, relative to `root`. Symlinks are skipped
/// so an export never follows a link out of the session directory.
fn collect_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            let skipped = directory == root
                && SKIPPED_DIRECTORIES
                    .iter()
                    .any(|name| entry.file_name() == std::ffi::OsStr::new(name));
            if !skipped {
                collect_files(root, &path, files)?;
            }
        } else if kind.is_file() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // Lock files are coordination state, not session evidence.
            if !name.ends_with(".lock") {
                files.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
            }
        }
    }
    Ok(())
}

/// A guide at the archive root, so an agent on another machine knows what it
/// has been handed and where to look first.
fn export_readme(session_id: &str, files: &[PathBuf]) -> String {
    let listing = files
        .iter()
        .map(|path| format!("- `{}`", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "# RustCode session export\n\n\
         - Session: `{session_id}`\n\
         - RustCode: v{version}\n\
         - Exported from: {os}/{arch}\n\n\
         ## Where to look\n\n\
         - `history.json`: the full transcript as a JSON array of messages \
         (`role` is user, assistant, tool or system; assistant messages carry \
         `tool_calls`, tool messages answer them).\n\
         - `logs/debug.log`: the engine log. Lines tagged `[op]` are structured \
         operational events; `turn.summary` gives each turn's rounds, tool \
         calls, tokens, cache use and `stop_reason`.\n\
         - `performance.json`: telemetry for the last turn.\n\
         - `metadata.json`, `title.txt`, `settings.json`: session identity, \
         title and the model/settings snapshots it ran with.\n\n\
         ## Files\n\n{listing}\n",
        version = env!("CARGO_PKG_VERSION"),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_download_dir_from_user_dirs() {
        let contents = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Nedlastinger\"\n";
        assert_eq!(
            user_dirs_download_entry(contents).as_deref(),
            Some("$HOME/Nedlastinger")
        );
        assert_eq!(
            user_dirs_download_entry("XDG_MUSIC_DIR=\"$HOME/Music\""),
            None
        );
    }

    #[test]
    fn archives_session_files_without_the_sandbox() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("history.json"), "[]").unwrap();
        std::fs::create_dir_all(source.path().join("logs")).unwrap();
        std::fs::write(source.path().join("logs/debug.log"), "[op] turn.summary").unwrap();
        std::fs::create_dir_all(source.path().join("sandbox")).unwrap();
        std::fs::write(source.path().join("sandbox/scratch.txt"), "x").unwrap();
        std::fs::write(source.path().join("history.json.lock"), "").unwrap();

        let out = tempfile::tempdir().unwrap();
        let archive_path = out.path().join("export.zip");
        write_archive("abc", source.path(), &archive_path).unwrap();

        let mut archive =
            zip::ZipArchive::new(std::fs::File::open(&archive_path).unwrap()).unwrap();
        let mut names = (0..archive.len())
            .map(|index| archive.by_index(index).unwrap().name().to_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            [
                "rustcode-session-abc/EXPORT.md",
                "rustcode-session-abc/history.json",
                "rustcode-session-abc/logs/debug.log",
            ]
        );
        assert!(!out.path().join("export.zip.partial").exists());
    }

    #[test]
    fn rejects_ids_that_are_paths() {
        assert!(export_session("../etc", None).is_err());
        assert!(export_session("", None).is_err());
    }
}
