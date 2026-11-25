//! Durable whole-file replacement: the one way RustCode writes a file that a
//! crash must never leave empty or half-written.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `bytes` to `path` so a reader sees either the old file or the whole
/// new one, never a truncated mix, even after a crash or power loss: a
/// temporary file beside it that no other writer shares, flushed to disk,
/// renamed over `path`, and on Unix the directory entry flushed too.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = temporary_path(path);
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|()| replace_file(&temporary, path));
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    sync_parent(path);
    Ok(())
}

/// Replace `destination` with `source`, preserving the old destination while
/// the replacement is being installed on platforms that cannot rename over an
/// existing file.
pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
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

/// A temporary name beside `path` that is unique to this process and call,
/// so two writers of the same file never write into each other's temporary.
fn temporary_path(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.{}.{sequence}.tmp", std::process::id()))
}

fn backup_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("file");
    destination.with_file_name(format!(".{name}.rustcode-backup-{}", std::process::id()))
}

/// Make the rename itself durable. Best effort: the file is already in place.
fn sync_parent(path: &Path) {
    if cfg!(unix)
        && let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
    {
        let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

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

    #[test]
    fn writes_creates_parents_and_leaves_no_temporary() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("a").join("b").join("file.json");

        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["file.json"]);
    }

    #[test]
    fn concurrent_writers_never_leave_a_partial_file() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = Arc::new(directory.path().join("shared.json"));
        let writers = 8;
        let barrier = Arc::new(Barrier::new(writers));
        let handles: Vec<_> = (0..writers)
            .map(|writer| {
                let path = Arc::clone(&path);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let body = vec![b'a' + writer as u8; 256 * 1024];
                    barrier.wait();
                    for _ in 0..8 {
                        write_atomic(&path, &body).unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let written = fs::read(&*path).unwrap();
        assert_eq!(written.len(), 256 * 1024);
        assert!(written.iter().all(|byte| *byte == written[0]));
    }

    #[test]
    fn a_failed_write_keeps_the_old_file_and_no_temporary() {
        let directory = tempfile::TempDir::new().unwrap();
        let path = directory.path().join("file.json");
        fs::write(&path, b"old").unwrap();
        let under_a_file = path.join("child.json");

        assert!(write_atomic(&under_a_file, b"new").is_err());

        assert_eq!(fs::read(&path).unwrap(), b"old");
        let temporaries = fs::read_dir(directory.path())
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(temporaries, 0);
    }
}
