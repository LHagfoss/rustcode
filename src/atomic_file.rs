use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Replace `destination` with `source`, preserving the old destination while
/// the replacement is being installed on platforms that cannot rename over an
/// existing file.
pub(crate) fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(_error) if cfg!(windows) => {
            let backup = backup_path(destination);
            let destination_exists = destination.exists();
            if destination_exists {
                fs::rename(destination, &backup)?;
            }

            match fs::rename(source, destination) {
                Ok(()) => {
                    if destination_exists {
                        let _ = fs::remove_file(backup);
                    }
                    Ok(())
                }
                Err(replace_error) => {
                    if destination_exists {
                        let _ = fs::rename(&backup, destination);
                    }
                    Err(replace_error)
                }
            }
        }
        Err(error) => Err(error),
    }
}

fn backup_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    destination.with_file_name(format!(".{name}.rustcode-backup-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();

        replace_file(&source, &destination).unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!source.exists());
    }

    #[test]
    fn replacement_failure_keeps_existing_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let source = directory.path().join("missing-source");
        let destination = directory.path().join("destination");
        fs::write(&destination, b"old").unwrap();

        assert!(replace_file(&source, &destination).is_err());

        assert_eq!(fs::read(&destination).unwrap(), b"old");
    }
}
