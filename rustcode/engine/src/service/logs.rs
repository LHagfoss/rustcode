//! Reading the server's log: the last lines of it, then (with `-f`) what is
//! appended, following it across a rotation.

use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// The last `lines` lines of the file, whole, and where it ended. A missing
/// file has no lines.
pub fn tail(path: &Path, lines: usize) -> io::Result<(String, u64)> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((String::new(), 0)),
        Err(error) => return Err(error),
    };
    let end = file.metadata()?.len();
    if lines == 0 {
        return Ok((String::new(), end));
    }
    // Read blocks from the end until there are more newlines than lines
    // wanted (the first is the previous line's end, or the file's start).
    const BLOCK: u64 = 64 * 1024;
    let mut start = end;
    let mut bytes = Vec::new();
    while start > 0 && bytes.iter().filter(|byte| **byte == b'\n').count() <= lines {
        let next = start.saturating_sub(BLOCK);
        let mut block = vec![0; (start - next) as usize];
        file.seek(SeekFrom::Start(next))?;
        file.read_exact(&mut block)?;
        block.extend_from_slice(&bytes);
        bytes = block;
        start = next;
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut wanted = text.lines().collect::<Vec<_>>();
    if start > 0 {
        // The first line may be cut by the block boundary.
        wanted.remove(0);
    }
    let from = wanted.len().saturating_sub(lines);
    let mut out = wanted[from..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Ok((out, end))
}

/// Follows a growing file that may be rotated: renamed away and replaced.
pub struct Follower {
    path: PathBuf,
    offset: u64,
    identity: Option<u64>,
}

impl Follower {
    /// Follow `path` from `offset`, where [`tail`] stopped.
    pub fn new(path: &Path, offset: u64) -> Self {
        Self {
            path: path.to_owned(),
            offset,
            identity: std::fs::metadata(path)
                .ok()
                .and_then(|meta| identity(&meta)),
        }
    }

    /// What was appended since the last call; none while the file is missing.
    pub fn read_new(&mut self) -> io::Result<String> {
        let mut file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
            Err(error) => return Err(error),
        };
        let meta = file.metadata()?;
        let now = identity(&meta);
        if (now.is_some() && now != self.identity) || meta.len() < self.offset {
            // Rotated or truncated: it is a new file, read from its start.
            self.offset = 0;
        }
        self.identity = now;
        if meta.len() == self.offset {
            return Ok(String::new());
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // A line still being written is left for the next call.
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |last| last + 1);
        self.offset += complete as u64;
        Ok(String::from_utf8_lossy(&bytes[..complete]).into_owned())
    }
}

#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(meta.ino())
}

#[cfg(not(unix))]
fn identity(_: &std::fs::Metadata) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn append(path: &Path, text: &str) {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }

    #[test]
    fn the_last_lines_come_whole() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        append(&path, "one\ntwo\nthree\nfour\n");
        let (text, end) = tail(&path, 2).unwrap();
        assert_eq!(text, "three\nfour\n");
        assert_eq!(end, 19);
        assert_eq!(tail(&path, 10).unwrap().0, "one\ntwo\nthree\nfour\n");
        assert_eq!(tail(&path, 0).unwrap().0, "");
    }

    #[test]
    fn a_long_file_is_read_from_its_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        let body: String = (0..20_000).map(|n| format!("line {n}\n")).collect();
        append(&path, &body);
        let (text, _) = tail(&path, 3).unwrap();
        assert_eq!(text, "line 19997\nline 19998\nline 19999\n");
    }

    #[test]
    fn a_missing_log_has_no_lines() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            tail(&dir.path().join("none"), 5).unwrap(),
            (String::new(), 0)
        );
    }

    #[test]
    fn following_reads_appended_lines_across_a_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        append(&path, "old\n");
        let (_, end) = tail(&path, 10).unwrap();
        let mut follower = Follower::new(&path, end);
        assert_eq!(follower.read_new().unwrap(), "");
        append(&path, "new\npart");
        assert_eq!(follower.read_new().unwrap(), "new\n");
        append(&path, "ial\n");
        assert_eq!(follower.read_new().unwrap(), "partial\n");

        // Rotated: renamed away, a new file in its place.
        std::fs::rename(&path, dir.path().join("server.log.1")).unwrap();
        assert_eq!(follower.read_new().unwrap(), "");
        append(&path, "after rotation, longer than before\n");
        assert_eq!(
            follower.read_new().unwrap(),
            "after rotation, longer than before\n"
        );
    }
}
