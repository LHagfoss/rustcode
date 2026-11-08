//! Settings › Diagnostics: what the shell knows about the RustCode server
//! (the `rustcode --acp` process `engine.rs` runs), the machine and the web
//! view, for debugging RustCode on macOS, Linux and Windows. Every command
//! here reads; the two that act open a folder the shell picks itself or save
//! to a file the user picks in the save panel, never a path from the page.

use crate::engine::{Engine, RunStatus};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::State;

/// The engine's config folder, resolved as `config::get_config_dir` does in
/// `rustcode/engine`: `$RUSTCODE_CONFIG_DIR`, else `%APPDATA%\rustcode` on
/// Windows, else `$XDG_CONFIG_HOME/rustcode` or `~/.config/rustcode`.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_from(|key| std::env::var_os(key))
}

fn config_dir_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let set = |key: &str| var(key).filter(|v| !v.to_string_lossy().trim().is_empty());
    if let Some(dir) = set("RUSTCODE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let root = if cfg!(windows) {
        set("APPDATA")
            .or_else(|| set("LOCALAPPDATA"))
            .map(PathBuf::from)?
    } else {
        set("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| set("HOME").map(|home| PathBuf::from(home).join(".config")))?
    };
    Some(root.join("rustcode"))
}

/// The user's home folder (`HOME`, or `USERPROFILE` on Windows), for the report's redaction.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Runs a short, fixed command and answers its trimmed stdout, or None when
/// it fails or takes longer than `limit` (it is killed then).
fn quick_output(
    program: impl AsRef<std::ffi::OsStr>,
    args: &[&str],
    limit: Duration,
) -> Option<String> {
    use std::io::Read;
    let mut child = crate::process::command(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return status.success().then(|| out.trim().to_string());
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
}

/// `rustcode 1.21.0` → `1.21.0`.
fn version_from(output: &str) -> Option<String> {
    let line = output.lines().next()?.trim();
    let version = line.strip_prefix("rustcode").map(str::trim).unwrap_or(line);
    (!version.is_empty()).then(|| version.to_string())
}

/// The server binary's version, asked once per path (`rustcode --version`).
fn server_version(path: &Path) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(known) = cache.lock().unwrap().get(path) {
        return known.clone();
    }
    let version =
        quick_output(path, &["--version"], Duration::from_secs(5)).and_then(|o| version_from(&o));
    cache
        .lock()
        .unwrap()
        .insert(path.to_path_buf(), version.clone());
    version
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    #[serde(flatten)]
    run: RunStatus,
    version: Option<String>,
    config_dir: Option<String>,
    /// The server's own `debug.log`, in the config folder.
    log_file: Option<String>,
}

#[tauri::command(async)]
pub fn server_status(engine: State<'_, Arc<Engine>>) -> ServerInfo {
    let run = engine.status();
    let path = run
        .path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(crate::engine::resolve_engine);
    let config = config_dir();
    ServerInfo {
        version: server_version(&path),
        run: RunStatus {
            path: Some(path.display().to_string()),
            ..run
        },
        log_file: config
            .as_ref()
            .map(|d| d.join("debug.log").display().to_string()),
        config_dir: config.map(|d| d.display().to_string()),
    }
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SystemInfo {
    /// `macos`, `linux`, `windows`.
    os: String,
    /// The release as the system names it (`15.2`, `Ubuntu 24.04.1 LTS`, `10.0.22631.4317`).
    os_version: Option<String>,
    /// The kernel (Linux only).
    kernel: Option<String>,
    arch: String,
    app_version: String,
    tauri_version: String,
    /// `WKWebView`, `WebView2` or `WebKitGTK`.
    webview: String,
    webview_version: Option<String>,
    home: Option<String>,
    /// Environment that changes how the web view draws (Linux: the session type and WebKit's
    /// compositing switches), as `KEY=value`.
    graphics_env: Vec<String>,
}

pub fn webview_kind(os: &str) -> &'static str {
    match os {
        "macos" | "ios" => "WKWebView",
        "windows" => "WebView2",
        _ => "WebKitGTK",
    }
}

/// `PRETTY_NAME` from `/etc/os-release`, else `NAME VERSION_ID`.
fn os_release_name(text: &str) -> Option<String> {
    let mut fields = HashMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once('=') {
            fields.insert(key.trim(), value.trim().trim_matches('"').to_string());
        }
    }
    if let Some(pretty) = fields.get("PRETTY_NAME").filter(|v| !v.is_empty()) {
        return Some(pretty.clone());
    }
    let name = fields.get("NAME")?;
    Some(match fields.get("VERSION_ID") {
        Some(version) => format!("{name} {version}"),
        None => name.clone(),
    })
}

/// `Microsoft Windows [Version 10.0.22631.4317]` → `10.0.22631.4317`.
fn windows_version(ver: &str) -> Option<String> {
    let start = ver.find("Version ")? + "Version ".len();
    let rest = &ver[start..];
    let end = rest.find(']').unwrap_or(rest.len());
    let version = rest[..end].trim();
    (!version.is_empty()).then(|| version.to_string())
}

fn os_version() -> (Option<String>, Option<String>) {
    let limit = Duration::from_secs(3);
    if cfg!(target_os = "macos") {
        let product = quick_output("sw_vers", &["-productVersion"], limit);
        let build = quick_output("sw_vers", &["-buildVersion"], limit);
        let version = match (product, build) {
            (Some(p), Some(b)) => Some(format!("{p} ({b})")),
            (p, _) => p,
        };
        (version, None)
    } else if cfg!(windows) {
        (
            quick_output("cmd", &["/C", "ver"], limit).and_then(|v| windows_version(&v)),
            None,
        )
    } else {
        let name = std::fs::read_to_string("/etc/os-release")
            .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
            .ok()
            .and_then(|t| os_release_name(&t));
        (name, quick_output("uname", &["-r"], limit))
    }
}

/// What changes how WebKitGTK composites, when it is set.
const GRAPHICS_ENV: &[&str] = &[
    "XDG_SESSION_TYPE",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "WEBKIT_DISABLE_COMPOSITING_MODE",
    "WEBKIT_DISABLE_DMABUF_RENDERER",
    "GDK_BACKEND",
    "LIBGL_ALWAYS_SOFTWARE",
    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
];

#[tauri::command(async)]
pub fn system_info(app: tauri::AppHandle) -> SystemInfo {
    let os = std::env::consts::OS.to_string();
    let (os_version, kernel) = os_version();
    SystemInfo {
        webview: webview_kind(&os).to_string(),
        os,
        os_version,
        kernel,
        arch: std::env::consts::ARCH.to_string(),
        app_version: app.package_info().version.to_string(),
        tauri_version: tauri::VERSION.to_string(),
        webview_version: tauri::webview_version().ok(),
        home: home_dir().map(|h| h.display().to_string()),
        graphics_env: GRAPHICS_ENV
            .iter()
            .filter_map(|key| {
                let value = std::env::var(key).ok()?;
                Some(format!("{key}={value}"))
            })
            .collect(),
    }
}

/// A bare program name only: no folders, no shell syntax.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.+@".contains(c))
        && name != "."
        && name != ".."
}

/// Where `name` resolves on `path` (a `PATH` value), trying each of `exts`
/// (`PATHEXT` on Windows, just "" elsewhere). An absolute name that exists is itself.
fn resolve_on(name: &str, path: &std::ffi::OsStr, exts: &[String]) -> Option<PathBuf> {
    let given = Path::new(name);
    if given.is_absolute() {
        return given.is_file().then(|| given.to_path_buf());
    }
    if !plain_name(name) {
        return None;
    }
    for dir in std::env::split_paths(path) {
        for ext in exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn path_exts() -> Vec<String> {
    let mut exts = vec![String::new()];
    if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        exts.extend(
            pathext
                .split(';')
                .filter(|e| !e.is_empty())
                .map(|e| e.to_ascii_lowercase()),
        );
    }
    exts
}

/// Each harness command's binary as this app's `PATH` finds it (None when it does not).
/// The server may see a fuller `PATH` than an app started from the Dock or a launcher.
#[tauri::command(async)]
pub fn resolve_binaries(names: Vec<String>) -> Vec<Option<String>> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let exts = path_exts();
    names
        .iter()
        .take(64)
        .map(|name| resolve_on(name, &path, &exts).map(|p| p.display().to_string()))
        .collect()
}

/// Shows the server's `debug.log` in the file manager (the config folder when there is none yet).
#[tauri::command(async)]
pub fn logs_folder_open() -> Result<(), String> {
    let dir = config_dir().ok_or("No config folder.")?;
    let log = dir.join("debug.log");
    if log.is_file() {
        return crate::native::reveal_path(log.display().to_string());
    }
    if !dir.is_dir() {
        return Err(format!("{} does not exist yet.", dir.display()));
    }
    crate::native::open_checked(&dir, true)
}

/// The largest text the save panel writes (a report or a log is far smaller).
const MAX_SAVE_BYTES: usize = 16 * 1024 * 1024;

/// A file name the save panel may suggest: one name, no folders.
fn suggested_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | ':' | '\0'))
        .take(100)
        .collect();
    let clean = clean.trim().trim_start_matches('.').to_string();
    if clean.is_empty() {
        "rustcode-diagnostics.txt".into()
    } else {
        clean
    }
}

/// Saves a report or log where the user picks in the save panel; answers the path, or
/// None when they cancel. The page never names the folder.
#[tauri::command(async)]
pub fn diagnostics_save(
    app: tauri::AppHandle,
    name: String,
    text: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    if text.len() > MAX_SAVE_BYTES {
        return Err("Too much text to save.".into());
    }
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(suggested_name(&name))
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = picked.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&path, text).map_err(|e| format!("Could not save {}: {e}", path.display()))?;
    Ok(Some(path.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_config_folder_follows_the_engine() {
        assert_eq!(
            config_dir_from(env(&[("RUSTCODE_CONFIG_DIR", "/tmp/rc"), ("HOME", "/h")])),
            Some(PathBuf::from("/tmp/rc"))
        );
        // A blank override is no override, as the engine reads it.
        let blank = config_dir_from(env(&[
            ("RUSTCODE_CONFIG_DIR", " "),
            ("HOME", "/h"),
            ("APPDATA", "C:/Users/u/AppData/Roaming"),
        ]));
        if cfg!(windows) {
            assert_eq!(
                blank,
                Some(PathBuf::from("C:/Users/u/AppData/Roaming/rustcode"))
            );
        } else {
            assert_eq!(blank, Some(PathBuf::from("/h/.config/rustcode")));
            assert_eq!(
                config_dir_from(env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")])),
                Some(PathBuf::from("/x/rustcode"))
            );
        }
        assert_eq!(config_dir_from(env(&[])), None);
    }

    #[test]
    fn versions_are_read_from_their_commands() {
        assert_eq!(version_from("rustcode 1.21.0\n").as_deref(), Some("1.21.0"));
        assert_eq!(version_from("1.2.3").as_deref(), Some("1.2.3"));
        assert_eq!(version_from(""), None);
        assert_eq!(
            windows_version("\r\nMicrosoft Windows [Version 10.0.22631.4317]\r\n").as_deref(),
            Some("10.0.22631.4317")
        );
        assert_eq!(windows_version("nothing"), None);
    }

    #[test]
    fn os_release_names_the_distribution() {
        let ubuntu = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n";
        assert_eq!(
            os_release_name(ubuntu).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(
            os_release_name("NAME=Arch\nVERSION_ID=rolling").as_deref(),
            Some("Arch rolling")
        );
        assert_eq!(os_release_name("ID=x"), None);
    }

    #[test]
    fn each_system_names_its_web_view() {
        assert_eq!(webview_kind("macos"), "WKWebView");
        assert_eq!(webview_kind("windows"), "WebView2");
        assert_eq!(webview_kind("linux"), "WebKitGTK");
    }

    #[test]
    fn binaries_resolve_on_the_path_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("rc-diag-bin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tool = dir.join("rc-tool.exe");
        std::fs::write(&tool, "").unwrap();
        let path = std::env::join_paths([&dir]).unwrap();
        let exts = vec![String::new(), ".exe".to_string()];
        assert_eq!(resolve_on("rc-tool", &path, &exts), Some(tool.clone()));
        assert_eq!(resolve_on("rc-tool.exe", &path, &exts), Some(tool.clone()));
        assert_eq!(resolve_on("missing", &path, &exts), None);
        // Folders and shell syntax are not program names.
        assert_eq!(resolve_on("../rc-tool", &path, &exts), None);
        assert_eq!(resolve_on("a;b", &path, &exts), None);
        assert_eq!(
            resolve_on(&tool.display().to_string(), &path, &exts),
            Some(tool)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_suggested_name_has_no_folders() {
        assert_eq!(suggested_name("../../etc/passwd"), "etcpasswd");
        assert_eq!(suggested_name("report.md"), "report.md");
        assert_eq!(suggested_name("  "), "rustcode-diagnostics.txt");
        assert_eq!(suggested_name(".hidden"), "hidden");
    }
}
