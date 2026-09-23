use serde_json::Value;
use std::path::Path;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

/// Keep each active debug log bounded. The current and previous files are
/// retained (`debug.log` and `debug.log.1`).
const MAX_DEBUG_LOG_BYTES: u64 = 50 * 1024 * 1024;
/// Keep all per-session debug logs together below the 240 MiB allowance.
const MAX_SESSION_LOGS_BYTES: u64 = 240 * 1024 * 1024;
/// Recheck the aggregate budget after each 16 MiB written in this process.
const SESSION_LOG_PRUNE_INTERVAL_BYTES: u64 = 16 * 1024 * 1024;
const OVERSIZED_LINE_MARKER: &str = "[logger] dropped oversized line";
const DEBUG_LOG_LOCK_NAME: &str = "debug.log.lock";
const ACTIVE_LOG_MARKERS_DIR: &str = ".active-log-locks";
const ROTATION_STAGE_NAME: &str = "debug.log.rotate.tmp";
const ROTATION_BACKUP_NAME: &str = "debug.log.1.rotate.bak";

pub(crate) fn set_active_session_id(session_id: Option<&str>) {
    let next = session_id.map(str::to_owned).filter(|id| !id.is_empty());
    let _guard = log_write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let changed = active_session_id() != next;
    if let Some(config_dir) = crate::config::get_config_dir()
        && let Some(_file_lock) = acquire_log_file_lock(&config_dir)
    {
        let marker_registered = update_active_session_marker(&config_dir, next.as_deref());
        set_active_session_id_memory(next.clone());
        if changed && (next.is_none() || marker_registered) {
            prune_session_logs(&config_dir, next.as_deref(), MAX_SESSION_LOGS_BYTES);
        }
        return;
    }

    set_active_session_id_memory(next);
    if active_session_id().is_none() {
        *active_session_marker_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
    }
}

fn active_session_id() -> Option<String> {
    active_session_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

fn active_session_lock() -> &'static Mutex<Option<String>> {
    static ACTIVE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

fn set_active_session_id_memory(session_id: Option<String>) {
    *active_session_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = session_id;
}

struct ActiveSessionMarker {
    config_dir: PathBuf,
    path: PathBuf,
    session_id: String,
    file: std::fs::File,
}

fn active_session_marker_lock() -> &'static Mutex<Option<ActiveSessionMarker>> {
    static MARKER: OnceLock<Mutex<Option<ActiveSessionMarker>>> = OnceLock::new();
    MARKER.get_or_init(|| Mutex::new(None))
}

fn active_log_markers_dir(config_dir: &Path) -> PathBuf {
    config_dir
        .join(rustcode_session::SESSIONS_DIR)
        .join(ACTIVE_LOG_MARKERS_DIR)
}

fn active_log_marker_path(config_dir: &Path, process_id: u32) -> PathBuf {
    active_log_markers_dir(config_dir).join(format!("{process_id}.lock"))
}

fn update_active_session_marker(config_dir: &Path, session_id: Option<&str>) -> bool {
    let marker_path = active_log_marker_path(config_dir, std::process::id());
    let mut active_marker = active_session_marker_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    if let Some(session_id) = session_id {
        let sessions_dir = config_dir.join(rustcode_session::SESSIONS_DIR);
        if std::fs::create_dir_all(&sessions_dir).is_err() || !is_real_directory(&sessions_dir) {
            return false;
        }
        let markers_dir = active_log_markers_dir(config_dir);
        if std::fs::create_dir_all(&markers_dir).is_err() || !is_real_directory(&markers_dir) {
            return false;
        }
        if let Some(marker) = active_marker.as_mut()
            && marker.config_dir == config_dir
            && marker.path == marker_path
            && is_real_file(&marker.path)
        {
            if marker.session_id == session_id {
                return true;
            }
            if write_active_session_marker(&mut marker.file, session_id).is_err() {
                return false;
            }
            marker.session_id = session_id.to_owned();
            return true;
        }

        let Some(file) = acquire_active_session_marker(config_dir, std::process::id(), session_id)
        else {
            return false;
        };
        *active_marker = Some(ActiveSessionMarker {
            config_dir: config_dir.to_path_buf(),
            path: marker_path,
            session_id: session_id.to_owned(),
            file,
        });
        true
    } else {
        if let Some(marker) = active_marker.as_mut()
            && marker.config_dir == config_dir
        {
            if write_active_session_marker(&mut marker.file, "").is_err() {
                return false;
            }
        }
        *active_marker = None;
        true
    }
}

fn acquire_active_session_marker(
    config_dir: &Path,
    process_id: u32,
    session_id: &str,
) -> Option<std::fs::File> {
    let markers_dir = active_log_markers_dir(config_dir);
    if std::fs::create_dir_all(&markers_dir).is_err()
        || !is_real_directory(&config_dir.join(rustcode_session::SESSIONS_DIR))
        || !is_real_directory(&markers_dir)
    {
        return None;
    }
    let mut file = open_active_marker_file(&active_log_marker_path(config_dir, process_id)).ok()?;
    // Shared locks let independent RustCode processes protect the same
    // session concurrently. The exclusive fallback still gives this process
    // a visible lock on filesystems that do not support shared locks.
    if file.try_lock_shared().is_err() && file.try_lock().is_err() {
        return None;
    }
    write_active_session_marker(&mut file, session_id).ok()?;
    Some(file)
}

fn open_active_marker_file(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !std::fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(std::io::Error::other(
            "active session marker is not a regular file",
        ));
    }
    Ok(file)
}

fn is_real_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn write_active_session_marker(file: &mut std::fs::File, session_id: &str) -> std::io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(encode_session_id(session_id).as_bytes())?;
    file.flush()
}

fn encode_session_id(session_id: &str) -> String {
    let mut encoded = String::with_capacity(session_id.len().saturating_mul(2));
    for byte in session_id.as_bytes() {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn decode_session_id(encoded: &str) -> Option<String> {
    if encoded.is_empty() || encoded.len() % 2 != 0 {
        return None;
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let pair = std::str::from_utf8(chunk).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect::<Option<Vec<_>>>()?;
    String::from_utf8(bytes).ok()
}

/// Rotate `debug.log` out of the way if it has grown past the size cap.
/// Also called before writes so a long-lived process cannot grow the global
/// or session log beyond the cap by accumulating lines between restarts.
pub(crate) fn rotate_if_oversized() {
    let _guard = log_write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(config_dir) = crate::config::get_config_dir() {
        let Some(_file_lock) = acquire_log_file_lock(&config_dir) else {
            return;
        };
        let active_session_id = active_session_id();
        let marker_registered =
            update_active_session_marker(&config_dir, active_session_id.as_deref());
        rotate_log_dir_if_oversized(&config_dir, MAX_DEBUG_LOG_BYTES);
        if let Some(session_id) = active_session_id.as_deref() {
            let session_dir =
                rustcode_session::SessionStore::new(&config_dir).session_dir(session_id);
            rotate_log_dir_if_oversized(&session_dir.join("logs"), MAX_DEBUG_LOG_BYTES);
        }
        if active_session_id.is_none() || marker_registered {
            prune_session_logs(
                &config_dir,
                active_session_id.as_deref(),
                MAX_SESSION_LOGS_BYTES,
            );
        }
    }
}

fn log_write_lock() -> &'static Mutex<()> {
    // The persistent sibling lock file serializes processes; this mutex also
    // serializes threads before they contend for that OS-level lock.
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn acquire_log_file_lock(config_dir: &Path) -> Option<std::fs::File> {
    use std::fs::OpenOptions;

    std::fs::create_dir_all(config_dir).ok()?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(config_dir.join(DEBUG_LOG_LOCK_NAME))
        .ok()?;
    lock.lock().ok()?;
    Some(lock)
}

fn try_acquire_log_file_lock(config_dir: &Path) -> Option<std::fs::File> {
    use std::fs::OpenOptions;

    std::fs::create_dir_all(config_dir).ok()?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(config_dir.join(DEBUG_LOG_LOCK_NAME))
        .ok()?;
    lock.try_lock().ok()?;
    Some(lock)
}

fn rotate_log_dir_if_oversized(log_dir: &std::path::Path, limit_bytes: u64) {
    let log_path = log_dir.join("debug.log");
    let rotated_path = log_dir.join("debug.log.1");
    if recover_rotation(log_dir).is_err() {
        return;
    }
    let _ = bound_log_file(&rotated_path, limit_bytes);
    let Ok(meta) = std::fs::metadata(&log_path) else {
        return;
    };
    if meta.len() > limit_bytes {
        let _ = replace_rotated_log(&log_path, &rotated_path, limit_bytes);
    }
}

fn bound_log_file(path: &Path, limit_bytes: u64) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let Ok(metadata) = std::fs::metadata(path) else {
        return Ok(());
    };
    if metadata.len() <= limit_bytes {
        return Ok(());
    }

    let marker = format!("[logger] earlier log bytes discarded to fit {limit_bytes}-byte cap\n");
    let marker_bytes = marker
        .len()
        .min(usize::try_from(limit_bytes).unwrap_or(usize::MAX));
    let tail_bytes = limit_bytes.saturating_sub(marker_bytes as u64);
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut tail_bytes_buffer = Vec::with_capacity(tail_bytes as usize);
    if tail_bytes > 0 {
        file.seek(SeekFrom::End(-(tail_bytes as i64)))?;
        file.read_to_end(&mut tail_bytes_buffer)?;
        if let Some(line_end) = tail_bytes_buffer.iter().position(|byte| *byte == b'\n')
            && line_end + 1 < tail_bytes_buffer.len()
        {
            tail_bytes_buffer.drain(..=line_end);
        }
    }
    let tail = valid_utf8_tail(&tail_bytes_buffer, tail_bytes as usize);

    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&marker.as_bytes()[..marker_bytes])?;
    file.write_all(tail.as_bytes())?;
    file.flush()
}

fn valid_utf8_tail(bytes: &[u8], max_bytes: usize) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    let mut start = decoded.len().saturating_sub(max_bytes);
    while !decoded.is_char_boundary(start) {
        start += 1;
    }
    decoded[start..].to_owned()
}

fn replace_rotated_log(
    log_path: &Path,
    rotated_path: &Path,
    limit_bytes: u64,
) -> std::io::Result<()> {
    let Some(log_dir) = log_path.parent() else {
        return Err(std::io::Error::other("debug log has no parent directory"));
    };
    recover_rotation(log_dir)?;
    replace_rotated_log_with(
        log_path,
        rotated_path,
        &log_dir.join(ROTATION_STAGE_NAME),
        &log_dir.join(ROTATION_BACKUP_NAME),
        limit_bytes,
        |from, to| std::fs::copy(from, to),
        |from, to| std::fs::rename(from, to),
        |path| std::fs::remove_file(path),
    )
}

fn recover_rotation(log_dir: &Path) -> std::io::Result<()> {
    recover_rotation_with(
        log_dir,
        |from, to| std::fs::rename(from, to),
        |path| std::fs::remove_file(path),
    )
}

fn recover_rotation_with<R, D>(
    log_dir: &Path,
    rename_file: R,
    remove_file: D,
) -> std::io::Result<()>
where
    R: Fn(&Path, &Path) -> std::io::Result<()>,
    D: Fn(&Path) -> std::io::Result<()>,
{
    let rotated_path = log_dir.join("debug.log.1");
    let log_path = log_dir.join("debug.log");
    let staged_path = log_dir.join(ROTATION_STAGE_NAME);
    let backup_path = log_dir.join(ROTATION_BACKUP_NAME);
    if backup_path.exists() {
        if !backup_path.is_file() {
            return Err(std::io::Error::other(
                "debug log rotation backup is not a regular file",
            ));
        }
        if rotated_path.exists() {
            if log_path.exists() {
                // The new archive is installed, but the source may still be
                // present if rotation was interrupted. Complete that step
                // before discarding the previous archive.
                remove_file_if_exists_with(&log_path, &remove_file)?;
            }
            remove_file_if_exists_with(&backup_path, &remove_file)?;
        } else {
            rename_file(&backup_path, &rotated_path)?;
        }
    }
    let _ = remove_file_if_exists_with(&staged_path, &remove_file);
    Ok(())
}

fn remove_file_if_exists_with<D>(path: &Path, remove_file: &D) -> std::io::Result<()>
where
    D: Fn(&Path) -> std::io::Result<()>,
{
    match remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn replace_rotated_log_with<C, R, D>(
    log_path: &Path,
    rotated_path: &Path,
    staged_path: &Path,
    backup_path: &Path,
    limit_bytes: u64,
    copy_file: C,
    rename_file: R,
    remove_file: D,
) -> std::io::Result<()>
where
    C: Fn(&Path, &Path) -> std::io::Result<u64>,
    R: Fn(&Path, &Path) -> std::io::Result<()>,
    D: Fn(&Path) -> std::io::Result<()>,
{
    remove_file_if_exists_with(staged_path, &remove_file)?;
    if let Err(error) = copy_file(log_path, staged_path) {
        let _ = remove_file(staged_path);
        return Err(error);
    }
    if let Err(error) = bound_log_file(staged_path, limit_bytes) {
        let _ = remove_file(staged_path);
        return Err(error);
    }

    let had_archive = rotated_path.exists();
    if had_archive {
        remove_file_if_exists_with(backup_path, &remove_file)?;
        // Both renames target absent paths, which works on Windows where
        // std::fs::rename cannot replace an existing destination.
        rename_file(rotated_path, backup_path)?;
    }
    if let Err(error) = rename_file(staged_path, rotated_path) {
        if had_archive {
            let _ = rename_file(backup_path, rotated_path);
        }
        let _ = remove_file(staged_path);
        return Err(error);
    }

    if let Err(error) = remove_file(log_path) {
        // Keep the active segment unchanged and retain the old archive backup.
        return Err(error);
    }
    if had_archive {
        let _ = remove_file(backup_path);
    }
    Ok(())
}

pub(crate) fn append_line(line: &str) {
    append_line_with_session(line, None);
}

fn append_line_with_session(line: &str, session_id: Option<&str>) {
    if let Some(log_dir) = crate::config::get_config_dir() {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let formatted = format!("[{now}] {line}");
        let session_id = session_id.map(str::to_owned).or_else(active_session_id);
        append_line_to_logs(&log_dir, session_id.as_deref(), &formatted);
    }
}

/// Write every line to the bounded config-level log, and duplicate it under
/// its owning session when the logger has an explicit or active session id.
fn append_line_to_logs(config_dir: &Path, session_id: Option<&str>, line: &str) {
    append_line_to_logs_with_limit(config_dir, session_id, line, MAX_DEBUG_LOG_BYTES);
}

fn append_line_to_logs_with_limit(
    config_dir: &Path,
    session_id: Option<&str>,
    line: &str,
    limit_bytes: u64,
) {
    if limit_bytes == 0 {
        return;
    }

    let _guard = log_write_lock()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let Some(_file_lock) = acquire_log_file_lock(config_dir) else {
        return;
    };
    let active_session_id = active_session_id();
    let marker_registered = if session_id.is_some() {
        update_active_session_marker(config_dir, active_session_id.as_deref())
    } else {
        true
    };
    let line = capped_log_line(line, limit_bytes);
    let append_bytes = line.len().saturating_add(1) as u64;
    append_bounded_line(config_dir, &line, append_bytes, limit_bytes);

    if let Some(session_id) = session_id {
        let session_dir = rustcode_session::SessionStore::new(config_dir).session_dir(session_id);
        let logs_dir = session_dir.join("logs");
        append_bounded_line(&logs_dir, &line, append_bytes, limit_bytes);
        if marker_registered {
            maybe_prune_session_logs(config_dir, append_bytes);
        }
    }
}

fn maybe_prune_session_logs(config_dir: &Path, bytes_written: u64) {
    maybe_prune_session_logs_with_limit(
        config_dir,
        active_session_id().as_deref(),
        bytes_written,
        MAX_SESSION_LOGS_BYTES,
    );
}

fn maybe_prune_session_logs_with_limit(
    config_dir: &Path,
    active_session_id: Option<&str>,
    bytes_written: u64,
    limit_bytes: u64,
) {
    // `append_line_to_logs_with_limit` holds the process and file locks, so a
    // small process-local counter avoids walking thousands of session folders
    // for every individual log event while still checking after bounded
    // amounts of new log data.
    static BYTES_SINCE_PRUNE: AtomicU64 = AtomicU64::new(0);
    let bytes_since_prune = BYTES_SINCE_PRUNE.fetch_add(bytes_written, Ordering::Relaxed);
    if bytes_since_prune.saturating_add(bytes_written) < SESSION_LOG_PRUNE_INTERVAL_BYTES {
        return;
    }
    BYTES_SINCE_PRUNE.store(0, Ordering::Relaxed);
    prune_session_logs(config_dir, active_session_id, limit_bytes);
}

#[derive(Debug, Clone)]
struct SessionLogFile {
    path: std::path::PathBuf,
    session_id: String,
    size: u64,
    rotated: bool,
    modified: SystemTime,
}

fn prune_session_logs(config_dir: &Path, active_session_id: Option<&str>, limit_bytes: u64) {
    let Some(protected_sessions) = active_session_ids(config_dir, active_session_id) else {
        // If a live marker cannot be inspected reliably, retain all session
        // logs instead of risking deletion from an active session.
        return;
    };
    let mut log_files = collect_session_log_files(config_dir);
    let mut total_bytes = log_files
        .iter()
        .fold(0_u64, |total, log| total.saturating_add(log.size));
    if total_bytes <= limit_bytes {
        return;
    }

    log_files.retain(|log| !protected_sessions.contains(&log.session_id));
    order_session_logs_for_pruning(&mut log_files);

    for log in log_files {
        if total_bytes <= limit_bytes {
            break;
        }
        if std::fs::remove_file(&log.path).is_ok() {
            total_bytes = total_bytes.saturating_sub(log.size);
        }
    }
}

fn active_session_ids(
    config_dir: &Path,
    local_active_session_id: Option<&str>,
) -> Option<std::collections::HashSet<String>> {
    let mut protected = std::collections::HashSet::new();
    if let Some(session_id) = local_active_session_id {
        protected.insert(session_id.to_owned());
    }

    let marker_dir = active_log_markers_dir(config_dir);
    match std::fs::symlink_metadata(&marker_dir) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(protected),
        Err(_) => return None,
        Ok(metadata) if !metadata.file_type().is_dir() => return None,
        Ok(_) => {}
    }
    let entries = match std::fs::read_dir(&marker_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(protected),
        Err(_) => return None,
    };
    for entry in entries {
        let Ok(entry) = entry else {
            return None;
        };
        let Ok(file_type) = entry.file_type() else {
            return None;
        };
        if !file_type.is_file()
            || !entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "lock")
        {
            continue;
        }

        let Ok(mut file) = OpenOptions::new().read(true).write(true).open(entry.path()) else {
            return None;
        };
        if file.try_lock().is_ok() {
            // No process holds a compatible active-session lock. Stable
            // marker files are intentionally left in place for reuse.
            continue;
        }

        let mut encoded_session_id = String::new();
        if file.seek(SeekFrom::Start(0)).is_err()
            || file.read_to_string(&mut encoded_session_id).is_err()
        {
            return None;
        }
        let Some(session_id) = decode_session_id(&encoded_session_id) else {
            return None;
        };
        protected.insert(session_id);
    }
    Some(protected)
}

fn order_session_logs_for_pruning(log_files: &mut [SessionLogFile]) {
    // Rotated segments contain older evidence than each session's current
    // segment, so discard those first. Within each class, age is primary and
    // the full path makes equal timestamps deterministic.
    log_files.sort_by(|left, right| {
        right
            .rotated
            .cmp(&left.rotated)
            .then_with(|| left.modified.cmp(&right.modified))
            .then_with(|| left.path.cmp(&right.path))
    });
}

fn collect_session_log_files(config_dir: &Path) -> Vec<SessionLogFile> {
    let sessions_dir = config_dir.join(rustcode_session::SESSIONS_DIR);
    let mut session_dirs = Vec::new();
    for top_level in child_directories(&sessions_dir) {
        if is_real_directory(&top_level.join("logs")) {
            // Legacy sessions/<id>/ layout.
            session_dirs.push(top_level);
            continue;
        }

        // Canonical sessions/YYYY/MM/DD/<id>/ layout. Walk only the known
        // directory depth so unrelated nested artifacts are never treated as
        // session roots.
        for month in child_directories(&top_level) {
            for day in child_directories(&month) {
                session_dirs.extend(
                    child_directories(&day)
                        .into_iter()
                        .filter(|session| is_real_directory(&session.join("logs"))),
                );
            }
        }
    }

    let mut log_files = Vec::new();
    for session_dir in session_dirs {
        let Some(session_id) = session_dir
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        let logs_dir = session_dir.join("logs");
        for name in ["debug.log", "debug.log.1"] {
            let path = logs_dir.join(name);
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_file() {
                continue;
            }
            log_files.push(SessionLogFile {
                path,
                session_id: session_id.clone(),
                size: metadata.len(),
                rotated: name == "debug.log.1",
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
            });
        }
    }
    log_files
}

fn child_directories(path: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let entry_path = entry.path();
            entry
                .file_type()
                .ok()
                .filter(|file_type| file_type.is_dir())
                .map(|_| entry_path)
        })
        .collect()
}

fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn try_append_panic_line(line: &str) {
    let Some(config_dir) = crate::config::get_config_dir() else {
        return;
    };
    let session_id = active_session_lock()
        .try_lock()
        .ok()
        .and_then(|active| active.clone());
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let formatted = format!("[{now}] {line}");
    let _ = try_append_line_to_logs_with_limit(
        &config_dir,
        session_id.as_deref(),
        &formatted,
        MAX_DEBUG_LOG_BYTES,
    );
}

fn try_append_line_to_logs_with_limit(
    config_dir: &Path,
    session_id: Option<&str>,
    line: &str,
    limit_bytes: u64,
) -> bool {
    if limit_bytes == 0 {
        return false;
    }
    let _guard = match log_write_lock().try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return false,
    };
    let Some(_file_lock) = try_acquire_log_file_lock(config_dir) else {
        return false;
    };
    let line = capped_log_line(line, limit_bytes);
    let append_bytes = line.len().saturating_add(1) as u64;
    append_bounded_line(config_dir, &line, append_bytes, limit_bytes);
    if let Some(session_id) = session_id {
        let logs_dir = rustcode_session::SessionStore::new(config_dir)
            .session_dir(session_id)
            .join("logs");
        append_bounded_line(&logs_dir, &line, append_bytes, limit_bytes);
    }
    true
}

fn capped_log_line<'a>(line: &'a str, limit_bytes: u64) -> std::borrow::Cow<'a, str> {
    let max_content_bytes = usize::try_from(limit_bytes.saturating_sub(1)).unwrap_or(usize::MAX);
    if line.len() <= max_content_bytes {
        return std::borrow::Cow::Borrowed(line);
    }

    let marker = format!(
        "{OVERSIZED_LINE_MARKER} ({} bytes)",
        line.len().saturating_add(1)
    );
    if marker.len() <= max_content_bytes {
        return std::borrow::Cow::Owned(marker);
    }

    // The marker is ASCII, so slicing at any byte boundary stays valid UTF-8.
    std::borrow::Cow::Owned(marker[..max_content_bytes].to_owned())
}

fn append_bounded_line(log_dir: &Path, line: &str, append_bytes: u64, limit_bytes: u64) {
    let log_path = log_dir.join("debug.log");
    let rotated_path = log_dir.join("debug.log.1");
    if recover_rotation(log_dir).is_err() {
        return;
    }
    let _ = bound_log_file(&rotated_path, limit_bytes);
    let current_bytes = std::fs::metadata(&log_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current_bytes.saturating_add(append_bytes) > limit_bytes {
        if replace_rotated_log(&log_path, &rotated_path, limit_bytes).is_err() {
            // The source is not modified unless a complete archive has been
            // installed, so a failed rotation leaves the existing segment.
            return;
        }
    }
    append_line_to_path(&log_path, line);
}

fn attributed_fields(mut fields: Value) -> (Value, Option<String>) {
    let session_id = fields
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or_else(active_session_id);
    if let Some(object) = fields.as_object_mut()
        && !object.contains_key("session_id")
        && let Some(session_id) = session_id.as_ref()
    {
        object.insert("session_id".to_owned(), Value::String(session_id.clone()));
    }
    (fields, session_id)
}

fn append_line_to_path(path: &Path, line: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Install a panic hook that preserves panic evidence in the global and, when
/// available, session-scoped logs.
/// Issue #1226: two sessions froze/died mid-stream with zero log evidence.
/// Unwind panics leave no macOS crash report, and a panic in a spawned task
/// is silently dropped unless its JoinHandle is observed — so without this
/// hook the next occurrence would be just as undiagnosable.
pub(crate) fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "<unknown location>".to_owned());
            // Never wait for a logger lock from the panic hook: a panic can
            // occur while the same thread is writing a log entry.
            try_append_panic_line(&format!("[PANIC] {payload} at {location}"));
            previous(info);
        }));
    });
}

/// Write metadata-only lifecycle events to the global and owning session logs.
pub(crate) fn operational_event(event: &str, fields: Value) {
    // Keep every operational event attributable even when a call site is in a
    // low-level stream/parser helper that does not otherwise carry session
    // state. Explicit ownership wins so a stale task cannot be attributed to
    // whichever session happens to be active when it unwinds.
    let (fields, session_id) = attributed_fields(fields);
    let payload = serde_json::json!({"event": event, "fields": fields});
    append_line_with_session(&format!("[op] {payload}"), session_id.as_deref());
}

#[macro_export]
macro_rules! dbg_log {
    ($($arg:tt)*) => {{
        $crate::logger::append_line(&format!($($arg)*));
    }};
}

#[macro_export]
macro_rules! dbg_log_for_session {
    ($session_id:expr, $($arg:tt)*) => {{
        $crate::logger::append_line_for_session(&$session_id, &format!($($arg)*));
    }};
}

pub(crate) fn append_line_for_session(session_id: &str, line: &str) {
    append_line_with_session(line, Some(session_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_triggers_when_log_exceeds_the_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        std::fs::write(&log_path, vec![b'x'; 200]).expect("write oversized log");

        // Small threshold so the test doesn't need to write anywhere near
        // the real 50MB cap.
        rotate_log_dir_if_oversized(dir.path(), 100);

        assert!(
            !log_path.exists(),
            "oversized debug.log should have been rotated away"
        );
        assert!(
            dir.path().join("debug.log.1").exists(),
            "rotated log should be preserved as debug.log.1"
        );
        assert!(
            std::fs::metadata(dir.path().join("debug.log.1"))
                .expect("rotated metadata")
                .len()
                <= 100,
            "oversized legacy logs must not leave an oversized archive"
        );
    }

    #[test]
    fn startup_bounds_an_oversized_existing_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        std::fs::write(&log_path, "small active log\n").expect("write active log");
        std::fs::write(
            &archive_path,
            format!("{}latest archived line\n", "x".repeat(150)),
        )
        .expect("write oversized archive");

        rotate_log_dir_if_oversized(dir.path(), 100);

        let archive = std::fs::read(&archive_path).expect("bounded archive");
        assert!(archive.len() <= 100);
        assert!(archive.ends_with(b"latest archived line\n"));
        assert!(log_path.exists(), "small active log should remain in place");
    }

    #[test]
    fn archive_cap_smaller_than_marker_remains_strict() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive_path = dir.path().join("debug.log.1");
        std::fs::write(&archive_path, vec![b'x'; 100]).expect("write archive");

        bound_log_file(&archive_path, 8).expect("bound tiny archive");

        assert_eq!(std::fs::metadata(archive_path).expect("metadata").len(), 8);
    }

    #[test]
    fn bounded_tail_never_starts_with_a_partial_utf8_character() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive_path = dir.path().join("debug.log.1");
        let limit_bytes = 64;
        let marker =
            format!("[logger] earlier log bytes discarded to fit {limit_bytes}-byte cap\n");
        let tail_bytes = limit_bytes - marker.len() as u64;
        let source = format!(
            "{}é{}",
            "a".repeat(100),
            "z".repeat(tail_bytes as usize - 1)
        );
        std::fs::write(&archive_path, source).expect("write utf8 archive");

        bound_log_file(&archive_path, limit_bytes).expect("bound utf8 archive");

        let archive = std::fs::read(&archive_path).expect("read bounded archive");
        assert!(archive.len() <= limit_bytes as usize);
        assert!(std::str::from_utf8(&archive).is_ok());
    }

    #[test]
    fn rotation_replaces_an_existing_archive_on_all_platforms() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        std::fs::write(&log_path, format!("{}new active tail\n", "n".repeat(110)))
            .expect("write oversized active log");
        std::fs::write(&archive_path, "old archive segment\n").expect("write old archive");

        rotate_log_dir_if_oversized(dir.path(), 100);

        let archive = std::fs::read(&archive_path).expect("new archive segment");
        assert!(archive.len() <= 100);
        assert!(archive.ends_with(b"new active tail\n"));
        assert!(!log_path.exists());
    }

    #[test]
    fn rotation_leaves_small_logs_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        std::fs::write(&log_path, vec![b'x'; 10]).expect("write small log");

        rotate_log_dir_if_oversized(dir.path(), 100);

        assert!(
            log_path.exists(),
            "a log under the size cap must not be rotated"
        );
        assert!(!dir.path().join("debug.log.1").exists());
    }

    #[test]
    fn rotation_is_a_no_op_when_no_log_exists_yet() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Must not panic or create anything when there's no debug.log yet
        // (e.g. the very first run of a fresh install).
        rotate_log_dir_if_oversized(dir.path(), 100);
        assert!(!dir.path().join("debug.log").exists());
        assert!(!dir.path().join("debug.log.1").exists());
    }

    #[test]
    fn writes_global_lines_when_no_session_is_active() {
        let dir = tempfile::tempdir().expect("tempdir");
        append_line_to_logs(dir.path(), None, "[time] early startup");
        let contents = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        assert!(contents.contains("early startup"));
        assert!(!dir.path().join("sessions").exists());
    }

    #[test]
    fn writes_session_lines_to_both_global_and_session_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        append_line_to_logs(
            dir.path(),
            Some(session_id),
            "[time] [op] {\"event\":\"turn.start\"}",
        );
        let global = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        let session_path = rustcode_session::SessionStore::new(dir.path())
            .session_dir(session_id)
            .join("logs/debug.log");
        let session = std::fs::read_to_string(session_path).expect("session log");
        assert!(global.contains("turn.start"));
        assert!(session.contains("turn.start"));
    }

    #[test]
    fn appends_at_exact_cap_boundary_for_global_and_session_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let session_dir = rustcode_session::SessionStore::new(dir.path()).session_dir(session_id);
        let session_logs = session_dir.join("logs");
        std::fs::create_dir_all(&session_logs).expect("create session log directory");
        std::fs::write(dir.path().join("debug.log"), b"old\n").expect("write global log");
        std::fs::write(session_logs.join("debug.log"), b"old\n").expect("write session log");

        append_line_to_logs_with_limit(dir.path(), Some(session_id), "new", 8);

        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log")).expect("current global log"),
            "old\nnew\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log")).expect("current session log"),
            "old\nnew\n"
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("debug.log"))
                .unwrap()
                .len(),
            8
        );
        assert_eq!(
            std::fs::metadata(session_logs.join("debug.log"))
                .unwrap()
                .len(),
            8
        );
        assert!(!dir.path().join("debug.log.1").exists());
        assert!(!session_logs.join("debug.log.1").exists());
    }

    #[test]
    fn rotates_before_an_append_that_would_cross_cap_for_both_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let session_dir = rustcode_session::SessionStore::new(dir.path()).session_dir(session_id);
        let session_logs = session_dir.join("logs");
        std::fs::create_dir_all(&session_logs).expect("create session log directory");
        std::fs::write(dir.path().join("debug.log"), b"old!\n").expect("write global log");
        std::fs::write(session_logs.join("debug.log"), b"old!\n").expect("write session log");

        append_line_to_logs_with_limit(dir.path(), Some(session_id), "new", 8);

        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log.1")).expect("rotated global log"),
            "old!\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log.1")).expect("rotated session log"),
            "old!\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("debug.log")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_logs.join("debug.log")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::metadata(dir.path().join("debug.log"))
                .unwrap()
                .len(),
            4
        );
        assert_eq!(
            std::fs::metadata(session_logs.join("debug.log"))
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn replaces_oversized_line_with_marker_that_fits_both_log_caps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let oversized = "x".repeat(300);

        append_line_to_logs_with_limit(dir.path(), Some(session_id), &oversized, 64);

        let global = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        let session_path = rustcode_session::SessionStore::new(dir.path())
            .session_dir(session_id)
            .join("logs/debug.log");
        let session = std::fs::read_to_string(session_path).expect("session log");
        assert!(global.starts_with(OVERSIZED_LINE_MARKER));
        assert!(session.starts_with(OVERSIZED_LINE_MARKER));
        assert!(!global.contains(&oversized));
        assert!(!session.contains(&oversized));
        assert!(global.len() <= 64);
        assert!(session.len() <= 64);
    }

    #[test]
    fn preserves_active_segments_when_archive_replacement_fails_for_both_log_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session_id = "2026-09-11T00:00:00Z-test-session";
        let session_dir = rustcode_session::SessionStore::new(dir.path()).session_dir(session_id);
        let session_logs = session_dir.join("logs");
        std::fs::create_dir_all(&session_logs).expect("create session log directory");
        let global_archive = dir.path().join("debug.log.1");
        let session_archive = session_logs.join("debug.log.1");
        std::fs::write(&global_archive, "global prior archive\n").expect("write global archive");
        std::fs::write(&session_archive, "session prior archive\n").expect("write session archive");
        std::fs::create_dir(dir.path().join(ROTATION_BACKUP_NAME))
            .expect("block global backup path");
        std::fs::create_dir(session_logs.join(ROTATION_BACKUP_NAME))
            .expect("block session backup path");
        std::fs::write(dir.path().join("debug.log"), vec![b'g'; 75]).expect("write global log");
        std::fs::write(session_logs.join("debug.log"), vec![b's'; 75]).expect("write session log");

        append_line_to_logs_with_limit(dir.path(), Some(session_id), "latest", 80);

        let global = std::fs::read_to_string(dir.path().join("debug.log")).expect("global log");
        let session = std::fs::read_to_string(session_logs.join("debug.log")).expect("session log");
        assert_eq!(global, "g".repeat(75));
        assert_eq!(session, "s".repeat(75));
        assert_eq!(
            std::fs::read_to_string(global_archive).unwrap(),
            "global prior archive\n"
        );
        assert_eq!(
            std::fs::read_to_string(session_archive).unwrap(),
            "session prior archive\n"
        );
    }

    #[test]
    fn rotation_copy_failure_preserves_active_and_previous_archive_segments() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        let staging_path = dir.path().join("debug.log.rotate.tmp");
        std::fs::write(&log_path, vec![b'a'; 75]).expect("write active segment");
        std::fs::write(&archive_path, "previous archive segment\n").expect("write archive");
        std::fs::create_dir(&staging_path).expect("block staging file creation");

        append_bounded_line(dir.path(), "latest", 7, 80);

        assert_eq!(
            std::fs::read_to_string(&archive_path).expect("old archive"),
            "previous archive segment\n"
        );
        assert_eq!(
            std::fs::read(&log_path).expect("retained active segment"),
            vec![b'a'; 75]
        );
    }

    #[test]
    fn staging_failure_leaves_active_and_archive_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        let staged_path = dir.path().join(ROTATION_STAGE_NAME);
        let backup_path = dir.path().join(ROTATION_BACKUP_NAME);
        std::fs::write(&log_path, "active segment\n").expect("write active");
        std::fs::write(&archive_path, "prior archive\n").expect("write archive");
        std::fs::create_dir(&staged_path).expect("block staging path");

        let result = replace_rotated_log_with(
            &log_path,
            &archive_path,
            &staged_path,
            &backup_path,
            100,
            |from, to| std::fs::copy(from, to),
            |from, to| std::fs::rename(from, to),
            |path| std::fs::remove_file(path),
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "active segment\n"
        );
        assert_eq!(
            std::fs::read_to_string(archive_path).unwrap(),
            "prior archive\n"
        );
    }

    #[test]
    fn staged_copy_failure_leaves_both_segments_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        let staged_path = dir.path().join(ROTATION_STAGE_NAME);
        let backup_path = dir.path().join(ROTATION_BACKUP_NAME);
        std::fs::write(&log_path, "active segment\n").expect("write active");
        std::fs::write(&archive_path, "prior archive\n").expect("write archive");
        let copy = |_: &Path, to: &Path| -> std::io::Result<u64> {
            std::fs::write(to, "partial staged bytes")?;
            Err(std::io::Error::other("injected stage copy failure"))
        };

        let result = replace_rotated_log_with(
            &log_path,
            &archive_path,
            &staged_path,
            &backup_path,
            100,
            copy,
            |from, to| std::fs::rename(from, to),
            |path| std::fs::remove_file(path),
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "active segment\n"
        );
        assert_eq!(
            std::fs::read_to_string(archive_path).unwrap(),
            "prior archive\n"
        );
        assert!(!staged_path.exists(), "partial stage should be removed");
    }

    #[test]
    fn archive_replacement_failure_restores_previous_archive() {
        use std::cell::Cell;

        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        let staged_path = dir.path().join(ROTATION_STAGE_NAME);
        let backup_path = dir.path().join(ROTATION_BACKUP_NAME);
        std::fs::write(&log_path, "active segment\n").expect("write active");
        std::fs::write(&archive_path, "prior archive\n").expect("write archive");
        let rename_count = Cell::new(0);
        let rename = |from: &Path, to: &Path| {
            let count = rename_count.get() + 1;
            rename_count.set(count);
            if count == 2 {
                Err(std::io::Error::other(
                    "injected archive replacement failure",
                ))
            } else {
                std::fs::rename(from, to)
            }
        };

        let result = replace_rotated_log_with(
            &log_path,
            &archive_path,
            &staged_path,
            &backup_path,
            100,
            |from, to| std::fs::copy(from, to),
            rename,
            |path| std::fs::remove_file(path),
        );

        assert!(result.is_err());
        assert_eq!(rename_count.get(), 3, "backup should be restored");
        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "active segment\n"
        );
        assert_eq!(
            std::fs::read_to_string(archive_path).unwrap(),
            "prior archive\n"
        );
        assert!(!backup_path.exists());
    }

    #[test]
    fn active_removal_failure_keeps_a_complete_segment_and_prior_archive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        let staged_path = dir.path().join(ROTATION_STAGE_NAME);
        let backup_path = dir.path().join(ROTATION_BACKUP_NAME);
        std::fs::write(&log_path, "active segment\n").expect("write active");
        std::fs::write(&archive_path, "prior archive\n").expect("write archive");
        let remove = |path: &Path| {
            if path == log_path {
                Err(std::io::Error::other("injected active removal failure"))
            } else {
                std::fs::remove_file(path)
            }
        };

        let result = replace_rotated_log_with(
            &log_path,
            &archive_path,
            &staged_path,
            &backup_path,
            100,
            |from, to| std::fs::copy(from, to),
            |from, to| std::fs::rename(from, to),
            remove,
        );

        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(log_path).unwrap(),
            "active segment\n"
        );
        assert_eq!(
            std::fs::read_to_string(archive_path).unwrap(),
            "active segment\n"
        );
        assert_eq!(
            std::fs::read_to_string(backup_path).unwrap(),
            "prior archive\n"
        );
    }

    #[test]
    fn recovery_keeps_previous_archive_until_active_log_removal_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("debug.log");
        let archive_path = dir.path().join("debug.log.1");
        let backup_path = dir.path().join(ROTATION_BACKUP_NAME);
        let staged_path = dir.path().join(ROTATION_STAGE_NAME);
        std::fs::write(&log_path, "new active segment\n").expect("write active");
        std::fs::write(&archive_path, "old archive\n").expect("write archive");

        let fail_active_removal = |path: &Path| {
            if path == log_path {
                Err(std::io::Error::other("injected active removal failure"))
            } else {
                std::fs::remove_file(path)
            }
        };
        let rotation = replace_rotated_log_with(
            &log_path,
            &archive_path,
            &staged_path,
            &backup_path,
            100,
            |from, to| std::fs::copy(from, to),
            |from, to| std::fs::rename(from, to),
            fail_active_removal,
        );
        assert!(rotation.is_err());
        assert_eq!(
            std::fs::read_to_string(&backup_path).unwrap(),
            "old archive\n"
        );

        let failed_recovery = recover_rotation_with(
            dir.path(),
            |from, to| std::fs::rename(from, to),
            |path| {
                if path == log_path {
                    Err(std::io::Error::other("injected recovery removal failure"))
                } else {
                    std::fs::remove_file(path)
                }
            },
        );
        assert!(failed_recovery.is_err());
        assert_eq!(
            std::fs::read_to_string(&log_path).unwrap(),
            "new active segment\n"
        );
        assert_eq!(
            std::fs::read_to_string(&archive_path).unwrap(),
            "new active segment\n"
        );
        assert_eq!(
            std::fs::read_to_string(&backup_path).unwrap(),
            "old archive\n"
        );

        recover_rotation(dir.path()).expect("finish interrupted rotation");
        assert!(!log_path.exists());
        assert_eq!(
            std::fs::read_to_string(&archive_path).unwrap(),
            "new active segment\n"
        );
        assert!(!backup_path.exists());
    }

    #[test]
    fn panic_logging_does_not_wait_for_a_busy_logger_mutex() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = log_write_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        assert!(!try_append_line_to_logs_with_limit(
            dir.path(),
            None,
            "panic while logger is busy",
            1024,
        ));
        assert!(!dir.path().join("debug.log").exists());
    }

    #[test]
    fn operational_fields_keep_explicit_session_ownership() {
        let (fields, session_id) = attributed_fields(serde_json::json!({
            "session_id": "older-session",
            "round": 2,
        }));
        assert_eq!(session_id.as_deref(), Some("older-session"));
        assert_eq!(fields["session_id"], "older-session");
    }

    #[test]
    fn aggregate_session_log_cap_prunes_oldest_inactive_segments_first() {
        let dir = tempfile::tempdir().expect("tempdir");
        let sessions = dir.path().join("sessions");
        let active_logs = sessions.join("active/logs");
        let old_session = sessions.join("old-session");
        let old_logs = old_session.join("logs");
        std::fs::create_dir_all(&active_logs).expect("active logs");
        std::fs::create_dir_all(&old_logs).expect("old logs");
        std::fs::write(active_logs.join("debug.log"), b"curr").expect("active current");
        std::fs::write(active_logs.join("debug.log.1"), b"arch").expect("active archive");
        std::fs::write(old_logs.join("debug.log"), b"curr").expect("old current");
        std::fs::write(old_logs.join("debug.log.1"), b"arch").expect("old archive");
        std::fs::write(old_session.join("history.json"), b"[]").expect("history");
        std::fs::write(old_session.join("metadata.json"), b"{}").expect("metadata");
        std::fs::write(old_session.join("artifact.bin"), b"artifact").expect("artifact");

        prune_session_logs(dir.path(), Some("active"), 12);

        let remaining_bytes = collect_session_log_files(dir.path())
            .iter()
            .map(|log| log.size)
            .sum::<u64>();
        assert_eq!(remaining_bytes, 12);
        assert!(active_logs.join("debug.log").exists());
        assert!(active_logs.join("debug.log.1").exists());
        assert!(old_logs.join("debug.log").exists());
        assert!(!old_logs.join("debug.log.1").exists());
        assert!(old_session.join("history.json").exists());
        assert!(old_session.join("metadata.json").exists());
        assert!(old_session.join("artifact.bin").exists());
        assert!(old_session.is_dir());
    }

    #[test]
    fn aggregate_session_log_cap_discovers_canonical_session_layout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let active_logs = dir.path().join("sessions/2026/09/23/active/logs");
        let old_logs = dir.path().join("sessions/2026/09/22/old/logs");
        std::fs::create_dir_all(&active_logs).expect("active logs");
        std::fs::create_dir_all(&old_logs).expect("old logs");
        std::fs::write(active_logs.join("debug.log"), b"active").expect("active log");
        std::fs::write(old_logs.join("debug.log"), b"inactive").expect("old log");

        prune_session_logs(dir.path(), Some("active"), 6);

        assert_eq!(collect_session_log_files(dir.path()).len(), 1);
        assert!(active_logs.join("debug.log").exists());
        assert!(!old_logs.join("debug.log").exists());
    }

    #[test]
    fn stale_session_log_write_cannot_override_active_session_protection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let active_logs = dir.path().join("sessions/current/logs");
        let stale_logs = dir.path().join("sessions/stale-owner/logs");
        std::fs::create_dir_all(&active_logs).expect("active logs");
        std::fs::create_dir_all(&stale_logs).expect("stale logs");
        std::fs::write(active_logs.join("debug.log"), b"curr").expect("active current");
        std::fs::write(active_logs.join("debug.log.1"), b"arch").expect("active archive");
        std::fs::write(stale_logs.join("debug.log"), b"stale").expect("stale owner log");

        let previous_active = active_session_id();
        *active_session_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some("current".to_owned());

        // A delayed async request can still log under stale-owner after the
        // UI has switched to current. The prune decision must consult the
        // process's active session, not the event's owning session.
        maybe_prune_session_logs_with_limit(
            dir.path(),
            active_session_id().as_deref(),
            SESSION_LOG_PRUNE_INTERVAL_BYTES,
            8,
        );

        assert!(active_logs.join("debug.log").exists());
        assert!(active_logs.join("debug.log.1").exists());
        assert!(!stale_logs.join("debug.log").exists());
        *active_session_lock()
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = previous_active;
    }

    #[test]
    fn cross_process_active_markers_protect_shared_session_logs_until_both_exit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let active_logs = dir.path().join("sessions/current/logs");
        let inactive_logs = dir.path().join("sessions/inactive/logs");
        std::fs::create_dir_all(&active_logs).expect("active logs");
        std::fs::create_dir_all(&inactive_logs).expect("inactive logs");
        std::fs::write(active_logs.join("debug.log"), b"curr").expect("active current");
        std::fs::write(active_logs.join("debug.log.1"), b"arch").expect("active archive");
        std::fs::write(inactive_logs.join("debug.log"), b"old!").expect("inactive current");

        // Distinct PID marker files model two RustCode processes holding the
        // same session active at once. Their shared file locks are independent
        // but both prevent the pruner from acquiring its exclusive probe.
        let owner_a = acquire_active_session_marker(dir.path(), 101, "current")
            .expect("first active process marker");
        let owner_b = acquire_active_session_marker(dir.path(), 202, "current")
            .expect("second active process marker");

        prune_session_logs(dir.path(), None, 8);

        assert!(active_logs.join("debug.log").exists());
        assert!(active_logs.join("debug.log.1").exists());
        assert!(!inactive_logs.join("debug.log").exists());

        drop((owner_a, owner_b));
        prune_session_logs(dir.path(), None, 4);

        assert!(active_logs.join("debug.log").exists());
        assert!(!active_logs.join("debug.log.1").exists());
    }

    #[test]
    fn session_log_pruning_order_is_age_then_path_with_rotated_files_first() {
        let older = UNIX_EPOCH + std::time::Duration::from_secs(1);
        let newer = UNIX_EPOCH + std::time::Duration::from_secs(2);
        let mut logs = vec![
            SessionLogFile {
                path: "sessions/z/debug.log.1".into(),
                session_id: "z".to_owned(),
                size: 1,
                rotated: true,
                modified: newer,
            },
            SessionLogFile {
                path: "sessions/b/debug.log.1".into(),
                session_id: "b".to_owned(),
                size: 1,
                rotated: true,
                modified: older,
            },
            SessionLogFile {
                path: "sessions/a/debug.log.1".into(),
                session_id: "a".to_owned(),
                size: 1,
                rotated: true,
                modified: older,
            },
            SessionLogFile {
                path: "sessions/c/debug.log".into(),
                session_id: "c".to_owned(),
                size: 1,
                rotated: false,
                modified: UNIX_EPOCH,
            },
        ];

        order_session_logs_for_pruning(&mut logs);

        let paths = logs
            .iter()
            .map(|log| log.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                "sessions/a/debug.log.1",
                "sessions/b/debug.log.1",
                "sessions/z/debug.log.1",
                "sessions/c/debug.log",
            ]
        );
    }
}
