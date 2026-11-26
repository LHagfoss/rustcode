//! Read-only detection of TCP listening ports scoped to a thread or worktree
//! folder, with optional project port labels and safe process termination.

use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One active TCP listener owned by a process scoped to a thread folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningPort {
    pub port: u16,
    pub pid: u32,
    pub address: String,
    pub process_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub url: String,
}

/// Snapshot of active listening ports for a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortList {
    pub cwd: String,
    pub ports: Vec<ListeningPort>,
}

/// Structured error returned when inspecting or stopping a listening process fails.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortError {
    pub code: &'static str,
    pub message: String,
}

impl PortError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Raw listener entry parsed from OS inspection before labeling and deduplication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPort {
    pub port: u16,
    pub pid: u32,
    pub address: String,
    pub process_name: String,
}

/// One row from a `/proc/net/tcp` or `/proc/net/tcp6` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcNetListener {
    pub port: u16,
    pub inode: u64,
    pub address: String,
}

/// One `(pid, ppid)` entry from the OS process table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub ppid: u32,
}

/// Parse a little-endian 8-hex-digit IPv4 address from `/proc/net/tcp`.
pub fn parse_ipv4_hex(hex: &str) -> Option<String> {
    if hex.len() != 8 {
        return None;
    }
    let b0 = u8::from_str_radix(&hex[6..8], 16).ok()?;
    let b1 = u8::from_str_radix(&hex[4..6], 16).ok()?;
    let b2 = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b3 = u8::from_str_radix(&hex[0..2], 16).ok()?;
    Some(format!("{b0}.{b1}.{b2}.{b3}"))
}

/// Parse a 32-hex-digit IPv6 address from `/proc/net/tcp6`.
pub fn parse_ipv6_hex(hex: &str) -> Option<String> {
    if hex.len() != 32 {
        return None;
    }
    let mut groups = Vec::with_capacity(8);
    for word_idx in 0..4 {
        let word = &hex[word_idx * 8..(word_idx + 1) * 8];
        let b0 = u8::from_str_radix(&word[6..8], 16).ok()?;
        let b1 = u8::from_str_radix(&word[4..6], 16).ok()?;
        let b2 = u8::from_str_radix(&word[2..4], 16).ok()?;
        let b3 = u8::from_str_radix(&word[0..2], 16).ok()?;
        let high = u16::from_be_bytes([b0, b1]);
        let low = u16::from_be_bytes([b2, b3]);
        groups.push(format!("{high:x}"));
        groups.push(format!("{low:x}"));
    }
    Some(groups.join(":"))
}

/// Parse one `/proc/net/tcp` or `/proc/net/tcp6` line, keeping only `TCP_LISTEN` (`0A`).
pub fn parse_proc_net_line(line: &str, ipv6: bool) -> Option<ProcNetListener> {
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 10 || cols[3] != "0A" {
        return None;
    }
    let (hex_ip, hex_port) = cols[1].rsplit_once(':')?;
    let port = u16::from_str_radix(hex_port, 16).ok()?;
    if port == 0 {
        return None;
    }
    let inode = cols[9].parse::<u64>().ok()?;
    if inode == 0 {
        return None;
    }
    let address = if ipv6 {
        parse_ipv6_hex(hex_ip)?
    } else {
        parse_ipv4_hex(hex_ip)?
    };
    Some(ProcNetListener {
        port,
        inode,
        address,
    })
}

/// Parse `socket:[<inode>]` target strings from `/proc/<pid>/fd/*`.
pub fn parse_socket_inode(link: &str) -> Option<u64> {
    let inner = link.strip_prefix("socket:[")?.strip_suffix(']')?;
    let inode = inner.parse::<u64>().ok()?;
    (inode > 0).then_some(inode)
}

/// Parse a `/proc/<pid>/stat` line into `(pid, ppid)`, tolerating spaces and parentheses in `comm`.
pub fn parse_proc_stat(stat: &str) -> Option<ProcessEntry> {
    let close = stat.rfind(')')?;
    let before = stat[..close].trim();
    let pid_str = before.split_whitespace().next()?;
    let pid = pid_str.parse::<u32>().ok()?;
    let after = stat[close + 1..].trim();
    let mut fields = after.split_whitespace();
    let _state = fields.next()?;
    let ppid = fields.next()?.parse::<u32>().ok()?;
    Some(ProcessEntry { pid, ppid })
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN` output into raw listeners.
pub fn parse_lsof_listen_output(output: &str) -> Vec<RawPort> {
    let mut ports = Vec::new();
    for line in output.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 10 {
            continue;
        }
        let process_name = cols[0];
        let Ok(pid) = cols[1].parse::<u32>() else {
            continue;
        };
        let Some(name) = cols.get(cols.len().saturating_sub(2)) else {
            continue;
        };
        let Some((address, port)) = split_host_port(name) else {
            continue;
        };
        ports.push(RawPort {
            port,
            pid,
            address: if address == "*" {
                "0.0.0.0".to_owned()
            } else {
                address.to_owned()
            },
            process_name: process_name.to_owned(),
        });
    }
    ports
}

/// Parse `lsof -a -p <pids> -d cwd -Fn` machine-readable output into a `pid -> cwd` map.
pub fn parse_lsof_cwd_output(output: &str) -> HashMap<u32, PathBuf> {
    let mut map = HashMap::new();
    let mut current_pid: Option<u32> = None;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            current_pid = rest.trim().parse::<u32>().ok();
        } else if let Some(rest) = line.strip_prefix('n')
            && let Some(pid) = current_pid
            && !rest.trim().is_empty()
        {
            map.insert(pid, PathBuf::from(rest.trim()));
        }
    }
    map
}

/// Parse `ps -A -o pid=,ppid=` output into `(pid, ppid)` rows.
pub fn parse_ps_table(output: &str) -> Vec<ProcessEntry> {
    output
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace();
            let pid = cols.next()?.parse::<u32>().ok()?;
            let ppid = cols.next()?.parse::<u32>().ok()?;
            Some(ProcessEntry { pid, ppid })
        })
        .collect()
}

/// Parse Windows `netstat -ano` output into raw TCP listeners.
pub fn parse_netstat_output(output: &str) -> Vec<RawPort> {
    let mut ports = Vec::new();
    for line in output.lines() {
        if !line.contains("LISTENING") {
            continue;
        }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        let Ok(pid) = cols[cols.len() - 1].parse::<u32>() else {
            continue;
        };
        let Some((address, port)) = split_host_port(cols[1]) else {
            continue;
        };
        ports.push(RawPort {
            port,
            pid,
            address: address.to_owned(),
            process_name: "process".to_owned(),
        });
    }
    ports
}

fn split_host_port(endpoint: &str) -> Option<(&str, u16)> {
    let (host, port_str) = if let Some(rest) = endpoint.strip_prefix('[') {
        let (addr, tail) = rest.split_once("]:")?;
        (addr, tail)
    } else {
        endpoint.rsplit_once(':')?
    };
    let port = port_str.parse::<u16>().ok()?;
    (port > 0 && !host.is_empty()).then_some((host, port))
}

/// Whether `candidate` equals `root` or lies strictly underneath it.
pub fn path_within(candidate: &Path, root: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

/// Select all PIDs whose own cwd or any ancestor's cwd is inside `root`, excluding `self_pid`.
pub fn scope_pids_to_dir(
    root: &Path,
    process_table: &[ProcessEntry],
    cwds: &HashMap<u32, PathBuf>,
    self_pid: u32,
) -> HashSet<u32> {
    let parent_of: HashMap<u32, u32> = process_table.iter().map(|e| (e.pid, e.ppid)).collect();
    let mut direct = HashSet::new();
    for (&pid, dir) in cwds {
        if pid != self_pid && path_within(dir, root) {
            direct.insert(pid);
        }
    }
    let mut scoped = direct.clone();
    for entry in process_table {
        if entry.pid == self_pid || scoped.contains(&entry.pid) {
            continue;
        }
        let mut cursor = entry.ppid;
        let mut hops = 0u32;
        while cursor > 1 && hops < 64 {
            if cursor == self_pid {
                break;
            }
            if direct.contains(&cursor) {
                scoped.insert(entry.pid);
                break;
            }
            let Some(&next) = parent_of.get(&cursor) else {
                break;
            };
            if next == cursor {
                break;
            }
            cursor = next;
            hops += 1;
        }
    }
    scoped
}

/// Build the browser URL for a listening `(address, port)`.
pub fn build_port_url(address: &str, port: u16) -> String {
    let is_local = matches!(
        address,
        "*" | "0.0.0.0" | "127.0.0.1" | "::" | "::1" | "0:0:0:0:0:0:0:0" | "0:0:0:0:0:0:0:1"
    );
    if is_local {
        format!("http://localhost:{port}")
    } else if address.contains(':') {
        format!("http://[{address}]:{port}")
    } else {
        format!("http://{address}:{port}")
    }
}

/// Deduplicate dual-stack listeners by port, attach optional labels, and sort ascending.
pub fn finalize_ports(raw: Vec<RawPort>, labels: &BTreeMap<u16, String>) -> Vec<ListeningPort> {
    let mut by_port: BTreeMap<u16, RawPort> = BTreeMap::new();
    for entry in raw {
        match by_port.get(&entry.port) {
            None => {
                by_port.insert(entry.port, entry);
            }
            Some(existing) => {
                let existing_ipv6 = existing.address.contains(':');
                let candidate_ipv6 = entry.address.contains(':');
                if (existing_ipv6 && !candidate_ipv6)
                    || (existing_ipv6 == candidate_ipv6 && entry.pid < existing.pid)
                {
                    by_port.insert(entry.port, entry);
                }
            }
        }
    }
    by_port
        .into_values()
        .map(|entry| {
            let url = build_port_url(&entry.address, entry.port);
            let label = labels.get(&entry.port).cloned();
            ListeningPort {
                port: entry.port,
                pid: entry.pid,
                address: entry.address,
                process_name: entry.process_name,
                label,
                url,
            }
        })
        .collect()
}

/// Parse optional port labels from TOML (`[ports]` table or `[[ports]]` array).
pub fn parse_port_labels_toml(raw: &str) -> BTreeMap<u16, String> {
    let mut labels = BTreeMap::new();
    let Ok(doc) = toml::from_str::<toml::Value>(raw) else {
        return labels;
    };
    if let Some(table) = doc.get("ports").and_then(toml::Value::as_table) {
        for (key, value) in table {
            if let Ok(port) = key.parse::<u16>()
                && port > 0
                && let Some(label) = value.as_str().map(str::trim).filter(|s| !s.is_empty())
            {
                labels.insert(port, label.to_owned());
            }
        }
    } else if let Some(items) = doc.get("ports").and_then(toml::Value::as_array) {
        for item in items {
            if let Some(port) = item
                .get("port")
                .and_then(toml::Value::as_integer)
                .and_then(|p| u16::try_from(p).ok())
                && port > 0
                && let Some(label) = item
                    .get("label")
                    .and_then(toml::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            {
                labels.insert(port, label.to_owned());
            }
        }
    }
    labels
}

/// Parse optional port labels from `.rustcode/ports.json` (`{ "ports": [{ "port", "label" }] }`).
pub fn parse_port_labels_json(raw: &str) -> BTreeMap<u16, String> {
    let mut labels = BTreeMap::new();
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(raw) else {
        return labels;
    };
    if let Some(items) = doc.get("ports").and_then(serde_json::Value::as_array) {
        for item in items {
            if let Some(port) = item
                .get("port")
                .and_then(serde_json::Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                && port > 0
                && let Some(label) = item
                    .get("label")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            {
                labels.insert(port, label.to_owned());
            }
        }
    }
    labels
}

fn load_project_port_labels(cwd: &Path) -> BTreeMap<u16, String> {
    let mut roots = vec![cwd.to_path_buf()];
    if let Ok(repo) = crate::worktrees::repo_root(cwd)
        && repo != cwd
    {
        roots.push(repo);
    }
    let mut merged = BTreeMap::new();
    for root in roots.into_iter().rev() {
        if let Ok(raw) = std::fs::read_to_string(root.join(".rustcode/worktrees.toml")) {
            merged.extend(parse_port_labels_toml(&raw));
        }
        if let Ok(raw) = std::fs::read_to_string(root.join(".rustcode/ports.toml")) {
            merged.extend(parse_port_labels_toml(&raw));
        }
        if let Ok(raw) = std::fs::read_to_string(root.join(".rustcode/ports.json")) {
            merged.extend(parse_port_labels_json(&raw));
        }
    }
    merged
}

#[cfg(target_os = "linux")]
fn scan_os_ports(root: &Path) -> Vec<RawPort> {
    let mut listeners = Vec::new();
    for (path, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
        if let Ok(content) = std::fs::read_to_string(path) {
            for line in content.lines().skip(1) {
                if let Some(parsed) = parse_proc_net_line(line, ipv6) {
                    listeners.push(parsed);
                }
            }
        }
    }
    if listeners.is_empty() {
        return Vec::new();
    }
    let wanted_inodes: HashSet<u64> = listeners.iter().map(|l| l.inode).collect();
    let mut table = Vec::new();
    let mut cwds = HashMap::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(pid) = name.parse::<u32>() else {
                continue;
            };
            let proc_dir = entry.path();
            if let Ok(stat) = std::fs::read_to_string(proc_dir.join("stat"))
                && let Some(row) = parse_proc_stat(&stat)
            {
                table.push(row);
            }
            if let Ok(link) = std::fs::read_link(proc_dir.join("cwd")) {
                let resolved = std::fs::canonicalize(&link).unwrap_or(link);
                cwds.insert(pid, resolved);
            }
        }
    }
    let scoped = scope_pids_to_dir(root, &table, &cwds, std::process::id());
    if scoped.is_empty() {
        return Vec::new();
    }
    let mut inode_to_pid: HashMap<u64, u32> = HashMap::new();
    let mut ordered_pids: Vec<u32> = scoped.into_iter().collect();
    ordered_pids.sort_unstable();
    for pid in ordered_pids {
        let fd_dir = PathBuf::from(format!("/proc/{pid}/fd"));
        let Ok(fds) = std::fs::read_dir(fd_dir) else {
            continue;
        };
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let Some(target_str) = target.to_str() else {
                continue;
            };
            if let Some(inode) = parse_socket_inode(target_str)
                && wanted_inodes.contains(&inode)
            {
                inode_to_pid.entry(inode).or_insert(pid);
            }
        }
    }
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut out = Vec::new();
    for listener in listeners {
        let Some(&pid) = inode_to_pid.get(&listener.inode) else {
            continue;
        };
        let process_name = names
            .entry(pid)
            .or_insert_with(|| {
                std::fs::read_to_string(format!("/proc/{pid}/comm"))
                    .ok()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "process".to_owned())
            })
            .clone();
        out.push(RawPort {
            port: listener.port,
            pid,
            address: listener.address,
            process_name,
        });
    }
    out
}

#[cfg(target_os = "macos")]
fn scan_os_ports(root: &Path) -> Vec<RawPort> {
    let output = match std::process::Command::new("lsof")
        .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    let all = parse_lsof_listen_output(&String::from_utf8_lossy(&output.stdout));
    if all.is_empty() {
        return Vec::new();
    }
    let ps_table = std::process::Command::new("ps")
        .args(["-A", "-o", "pid=,ppid="])
        .output()
        .ok()
        .map(|o| parse_ps_table(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default();
    let parent_of: HashMap<u32, u32> = ps_table.iter().map(|e| (e.pid, e.ppid)).collect();
    let mut pids_to_check: HashSet<u32> = HashSet::new();
    for entry in &all {
        pids_to_check.insert(entry.pid);
        let mut cursor = entry.pid;
        for _ in 0..8 {
            let Some(&ppid) = parent_of.get(&cursor) else {
                break;
            };
            if ppid <= 1 || ppid == cursor {
                break;
            }
            pids_to_check.insert(ppid);
            cursor = ppid;
        }
    }
    let pid_arg = pids_to_check
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    if pid_arg.is_empty() {
        return Vec::new();
    }
    let cwd_output = match std::process::Command::new("lsof")
        .args(["-a", "-p", &pid_arg, "-d", "cwd", "-Fn"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Vec::new(),
    };
    let raw_cwds = parse_lsof_cwd_output(&String::from_utf8_lossy(&cwd_output.stdout));
    let cwds: HashMap<u32, PathBuf> = raw_cwds
        .into_iter()
        .map(|(pid, path)| (pid, std::fs::canonicalize(&path).unwrap_or(path)))
        .collect();
    let scoped = scope_pids_to_dir(root, &ps_table, &cwds, std::process::id());
    all.into_iter()
        .filter(|entry| scoped.contains(&entry.pid))
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn scan_os_ports(_root: &Path) -> Vec<RawPort> {
    Vec::new()
}

/// List TCP ports currently listening from processes scoped to `cwd`.
pub fn list(cwd: &Path) -> Result<PortList, PortError> {
    if !cwd.is_dir() {
        return Err(PortError::new(
            "missing",
            format!("Folder does not exist: {}", cwd.display()),
        ));
    }
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let labels = load_project_port_labels(&canonical);
    let raw = scan_os_ports(&canonical);
    let ports = finalize_ports(raw, &labels);
    Ok(PortList {
        cwd: canonical.to_string_lossy().into_owned(),
        ports,
    })
}

#[cfg(unix)]
fn terminate_pid(pid: u32) -> Result<(), PortError> {
    let raw_pid = i32::try_from(pid)
        .map_err(|_| PortError::new("invalid", format!("Invalid process id {pid}")))?;
    if raw_pid <= 1 {
        return Err(PortError::new(
            "invalid",
            format!("Refusing to terminate system process {pid}"),
        ));
    }
    let rc = unsafe { libc::kill(raw_pid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(libc::ESRCH) {
            return Err(PortError::new(
                "failed",
                format!("Could not stop process {pid}: {err}"),
            ));
        }
        return Ok(());
    }
    for _ in 0..12 {
        std::thread::sleep(std::time::Duration::from_millis(50));
        let alive = unsafe { libc::kill(raw_pid, 0) } == 0;
        if !alive {
            return Ok(());
        }
    }
    let _ = unsafe { libc::kill(raw_pid, libc::SIGKILL) };
    std::thread::sleep(std::time::Duration::from_millis(50));
    Ok(())
}

#[cfg(windows)]
fn terminate_pid(pid: u32) -> Result<(), PortError> {
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
        .map_err(|error| {
            PortError::new("failed", format!("Could not stop process {pid}: {error}"))
        })?;
    if !status.success() {
        return Err(PortError::new(
            "failed",
            format!("taskkill exited with status {status}"),
        ));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn terminate_pid(pid: u32) -> Result<(), PortError> {
    Err(PortError::new(
        "unsupported",
        format!("Stopping process {pid} is not supported on this platform"),
    ))
}

/// Stop one listening process after confirming it is currently scoped to `cwd` on `port`.
pub fn kill(cwd: &Path, pid: u32, port: u16) -> Result<PortList, PortError> {
    if pid == std::process::id() {
        return Err(PortError::new(
            "forbidden",
            "Refusing to terminate the RustCode engine process",
        ));
    }
    let current = list(cwd)?;
    let owns = current
        .ports
        .iter()
        .any(|entry| entry.pid == pid && entry.port == port);
    if !owns {
        return Err(PortError::new(
            "not_found",
            format!("No listening process {pid} on port {port} belongs to this folder"),
        ));
    }
    terminate_pid(pid)?;
    list(cwd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_net_ipv4_and_ipv6_listen_lines() {
        let v4 = "   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 54321 1 00000000 100 0 0 10 0";
        assert_eq!(
            parse_proc_net_line(v4, false),
            Some(ProcNetListener {
                port: 3000,
                inode: 54321,
                address: "127.0.0.1".to_owned(),
            })
        );

        let established = "   1: 0100007F:0BB8 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 54322 1";
        assert_eq!(parse_proc_net_line(established, false), None);

        let v6 = "   0: 00000000000000000000000001000000:1435 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 99887 1";
        assert_eq!(
            parse_proc_net_line(v6, true),
            Some(ProcNetListener {
                port: 5173,
                inode: 99887,
                address: "0:0:0:0:0:0:0:1".to_owned(),
            })
        );
    }

    #[test]
    fn parses_proc_stat_with_parentheses_in_comm_and_socket_inodes() {
        let stat = "4242 (vite (dev)) S 4200 4200 4200 0 -1 4194304";
        assert_eq!(
            parse_proc_stat(stat),
            Some(ProcessEntry {
                pid: 4242,
                ppid: 4200,
            })
        );
        assert_eq!(parse_socket_inode("socket:[99887]"), Some(99887));
        assert_eq!(parse_socket_inode("pipe:[1234]"), None);
    }

    #[test]
    fn parses_lsof_listen_and_cwd_and_netstat_tables() {
        let lsof = "\
COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
node    12345 lucas  23u  IPv4 0x1234      0t0  TCP *:3000 (LISTEN)
bun     12346 lucas  18u  IPv6 0x5678      0t0  TCP [::1]:5173 (LISTEN)
";
        let parsed = parse_lsof_listen_output(lsof);
        assert_eq!(
            parsed,
            vec![
                RawPort {
                    port: 3000,
                    pid: 12345,
                    address: "0.0.0.0".to_owned(),
                    process_name: "node".to_owned(),
                },
                RawPort {
                    port: 5173,
                    pid: 12346,
                    address: "::1".to_owned(),
                    process_name: "bun".to_owned(),
                },
            ]
        );

        let cwds =
            parse_lsof_cwd_output("p12345\nfcwd\nn/code/worktrees/wt-a\np12346\nn/code/other\n");
        assert_eq!(
            cwds.get(&12345),
            Some(&PathBuf::from("/code/worktrees/wt-a"))
        );
        assert_eq!(cwds.get(&12346), Some(&PathBuf::from("/code/other")));

        let netstat = "  TCP    127.0.0.1:8080         0.0.0.0:0              LISTENING       777\n  TCP    [::]:8081              [::]:0                 LISTENING       778\n";
        let win = parse_netstat_output(netstat);
        assert_eq!(win.len(), 2);
        assert_eq!(win[0].port, 8080);
        assert_eq!(win[0].pid, 777);
        assert_eq!(win[1].port, 8081);
        assert_eq!(win[1].address, "::");
    }

    #[test]
    fn scopes_pids_to_worktree_dir_and_descendants_excluding_self() {
        let root = Path::new("/code/worktrees/wt-a");
        let table = vec![
            ProcessEntry { pid: 100, ppid: 1 },
            ProcessEntry {
                pid: 101,
                ppid: 100,
            },
            ProcessEntry { pid: 200, ppid: 1 },
            ProcessEntry { pid: 300, ppid: 1 },
        ];
        let mut cwds = HashMap::new();
        cwds.insert(100, PathBuf::from("/code/worktrees/wt-a/apps/web"));
        cwds.insert(101, PathBuf::from("/tmp"));
        cwds.insert(200, PathBuf::from("/code/worktrees/wt-b"));
        cwds.insert(300, PathBuf::from("/code/worktrees/wt-a"));

        let scoped = scope_pids_to_dir(root, &table, &cwds, 300);
        assert!(scoped.contains(&100));
        assert!(scoped.contains(&101));
        assert!(!scoped.contains(&200));
        assert!(!scoped.contains(&300));
    }

    #[test]
    fn parses_project_port_labels_and_deduplicates_dual_stack_ports() {
        let toml_labels = parse_port_labels_toml(
            r#"
[ports]
3000 = "Web"
5173 = "Vite"
"#,
        );
        assert_eq!(toml_labels.get(&3000).map(String::as_str), Some("Web"));
        assert_eq!(toml_labels.get(&5173).map(String::as_str), Some("Vite"));

        let json_labels = parse_port_labels_json(
            r#"{"ports":[{"port":8787,"label":"API"},{"port":0,"label":"Bad"}]}"#,
        );
        assert_eq!(json_labels.get(&8787).map(String::as_str), Some("API"));
        assert!(!json_labels.contains_key(&0));

        let finalized = finalize_ports(
            vec![
                RawPort {
                    port: 3000,
                    pid: 50,
                    address: "::".to_owned(),
                    process_name: "node".to_owned(),
                },
                RawPort {
                    port: 3000,
                    pid: 50,
                    address: "0.0.0.0".to_owned(),
                    process_name: "node".to_owned(),
                },
                RawPort {
                    port: 5173,
                    pid: 60,
                    address: "192.168.1.5".to_owned(),
                    process_name: "vite".to_owned(),
                },
            ],
            &toml_labels,
        );
        assert_eq!(finalized.len(), 2);
        assert_eq!(finalized[0].port, 3000);
        assert_eq!(finalized[0].address, "0.0.0.0");
        assert_eq!(finalized[0].label.as_deref(), Some("Web"));
        assert_eq!(finalized[0].url, "http://localhost:3000");
        assert_eq!(finalized[1].url, "http://192.168.1.5:5173");
    }
}
