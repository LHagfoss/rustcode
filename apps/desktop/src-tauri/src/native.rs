//! Native helpers behind narrow, validated commands.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Harness tools whose own login may be started in Terminal. Anything else is
/// refused, so a compromised page cannot run arbitrary commands.
const LOGIN_TOOLS: &[&str] = &[
    "codex",
    "claude",
    "gemini",
    "opencode",
    "claude-agent-acp",
    "codex-acp",
    "gh",
    "rustcode",
];

fn is_known_login_tool(tool: &str) -> bool {
    LOGIN_TOOLS.contains(&tool)
}

const MAX_DIFF_BYTES: usize = 2 * 1024 * 1024;

fn absolute_dir(path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_dir() {
        return Err(format!("Not a folder: {}", path.display()));
    }
    Ok(path)
}

#[tauri::command(async)]
pub fn open_url(url: String) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:"))
    {
        return Err("Only web and mail links open.".into());
    }
    open_with_system(&url)
}

/// Path a file manager should open or select for a reveal action.
///
/// macOS and Windows can select the requested item. On other systems, open a
/// directory itself, or the containing directory for a file.
fn reveal_target(path: &Path, can_select: bool) -> &Path {
    if can_select || path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    }
}

#[cfg(target_os = "macos")]
fn open_with_system(target: &str) -> Result<(), String> {
    crate::process::command("open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn open_with_system(target: &str) -> Result<(), String> {
    crate::process::command("explorer")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_with_system(target: &str) -> Result<(), String> {
    crate::process::command("xdg-open")
        .arg(target)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn reveal_path(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.exists() {
        return Err("No such file.".into());
    }
    #[cfg(target_os = "macos")]
    {
        crate::process::command("open")
            .arg("-R")
            .arg(reveal_target(&path, true))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        crate::process::command("explorer")
            .arg(format!("/select,{}", reveal_target(&path, true).display()))
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open Explorer: {e}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        open_with_system(&reveal_target(&path, false).display().to_string())
    }
}

#[cfg(target_os = "windows")]
fn interactive_command(program: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(program);
    command.creation_flags(0x0000_0010); // CREATE_NEW_CONSOLE
    command
}

/// POSIX single-quote a word for a shell command line.
fn shell_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// The environment a harness login may run with: only the variable naming the
/// login's own directory (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`).
const LOGIN_ENV: &[&str] = &["CLAUDE_CONFIG_DIR", "CODEX_HOME"];

/// Runs a harness's own login command in a new Terminal window, where the
/// user signs in directly with the harness. `env` is the login's directory,
/// for an account added in RustCode: it is validated here and prefixed to the
/// command, never run through a shell of its own. RustCode never sees the
/// result.
#[tauri::command(async)]
pub fn open_login_terminal(
    command: Vec<String>,
    env: Option<Vec<(String, String)>>,
) -> Result<(), String> {
    let tool = command.first().ok_or("No command.")?;
    if !is_known_login_tool(tool) {
        return Err(format!("{tool} is not a known harness login."));
    }
    if command.iter().any(|w| w.contains('\n') || w.contains('\r')) {
        return Err("Invalid command.".into());
    }
    let env = env.unwrap_or_default();
    for (key, value) in &env {
        if !LOGIN_ENV.contains(&key.as_str()) {
            return Err(format!("{key} is not a harness login directory."));
        }
        if value.contains('\n') || value.contains('\r') {
            return Err("Invalid environment.".into());
        }
    }
    let line = login_line(&command, &env);
    #[cfg(target_os = "macos")]
    {
        // AppleScript string: escape backslashes and quotes.
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
            line.replace('\\', "\\\\").replace('"', "\\\"")
        );
        crate::process::command("osascript")
            .arg("-e")
            .arg(script)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        let quote = |word: &str| format!("'{}'", word.replace('\'', "''"));
        let mut script = String::new();
        for (key, value) in &env {
            script.push_str(&format!("$env:{key} = {}; ", quote(value)));
        }
        script.push_str("& ");
        script.push_str(
            &command
                .iter()
                .map(|word| quote(word))
                .collect::<Vec<_>>()
                .join(" "),
        );
        let _ = line;
        interactive_command("powershell.exe")
            .args(["-NoExit", "-Command", &script])
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("Could not open PowerShell: {e}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = line;
        let mut args: Vec<String> = env
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        args.extend(command);
        for terminal in ["x-terminal-emulator", "gnome-terminal", "konsole"] {
            let separator = if terminal == "gnome-terminal" {
                "--"
            } else {
                "-e"
            };
            if crate::process::command(terminal)
                .arg(separator)
                .arg("env")
                .args(&args)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(
            "Could not open a terminal; install a supported terminal or copy the command instead."
                .into(),
        )
    }
}

/// The shell line a login runs: quoted environment assignments, then the
/// quoted command.
fn login_line(command: &[String], env: &[(String, String)]) -> String {
    let mut words: Vec<String> = env
        .iter()
        .map(|(key, value)| format!("{key}={}", shell_quote(value)))
        .collect();
    words.extend(command.iter().map(|word| shell_quote(word)));
    words.join(" ")
}

#[derive(Serialize)]
pub struct FileChange {
    path: String,
    status: String,
    #[serde(rename = "oldPath", skip_serializing_if = "Option::is_none")]
    old_path: Option<String>,
    added: u64,
    removed: u64,
    binary: bool,
}

#[derive(Serialize)]
pub struct GitChanges {
    branch: Option<String>,
    files: Vec<FileChange>,
    diff: String,
}

fn git(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::process::command("git")
        .arg("-C")
        .arg(cwd)
        .args(["-c", "core.quotepath=off"])
        .args(args)
        .output()
        .map_err(|e| format!("Could not run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A ref or commit the webview names as the base of a branch diff: no
/// option-like or odd characters, and it must resolve to a commit.
fn valid_base(base: &str) -> bool {
    !base.is_empty()
        && base.len() <= 200
        && !base.starts_with('-')
        && !base.contains("..")
        && base
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._/@~^-".contains(c))
}

/// How many files `@` references choose from.
const MAX_PROJECT_FILES: usize = 20_000;

/// The project's files for `@` references, as git sees them: tracked and
/// untracked, without what it ignores. Paths are relative to `cwd`.
#[tauri::command(async)]
pub fn project_files(cwd: String) -> Result<Vec<String>, String> {
    let cwd = absolute_dir(&cwd)?;
    let listed = git(&cwd, &["ls-files", "-co", "--exclude-standard"])?;
    Ok(listed
        .lines()
        .take(MAX_PROJECT_FILES)
        .map(str::to_owned)
        .collect())
}

/// Uncommitted changes against HEAD, plus untracked files. With `base`
/// (a worktree's base commit or branch), everything since the branch left
/// it: its commits and the working tree, against their merge base.
#[tauri::command(async)]
pub fn git_changes(cwd: String, base: Option<String>) -> Result<GitChanges, String> {
    let cwd = absolute_dir(&cwd)?;
    git(&cwd, &["rev-parse", "--is-inside-work-tree"])
        .map_err(|_| "This folder is not a git repository.".to_string())?;
    let branch = git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|b| b.trim().to_string());
    let has_head = git(&cwd, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let since = match base.as_deref() {
        Some(base) => {
            if !valid_base(base) {
                return Err("Not a valid base revision.".into());
            }
            let commit = format!("{base}^{{commit}}");
            git(&cwd, &["rev-parse", "--verify", "--quiet", &commit])
                .map_err(|_| format!("{base} is not a commit here."))?;
            Some(git(&cwd, &["merge-base", base, "HEAD"])?.trim().to_string())
        }
        None => None,
    };
    let base: Vec<&str> = match &since {
        Some(sha) => vec![sha.as_str()],
        None if has_head => vec!["HEAD"],
        None => vec![],
    };
    let base = base.as_slice();

    let mut files = Vec::new();
    let status = git(&cwd, &[&["diff", "--name-status", "-M"], base].concat())?;
    let numstat = git(&cwd, &[&["diff", "--numstat", "-M"], base].concat())?;
    let counts: Vec<(Option<u64>, Option<u64>)> = numstat
        .lines()
        .map(|l| {
            let mut parts = l.split('\t');
            (
                parts.next().and_then(|a| a.parse().ok()),
                parts.next().and_then(|r| r.parse().ok()),
            )
        })
        .collect();
    for (i, line) in status.lines().enumerate() {
        let parts: Vec<&str> = line.split('\t').collect();
        let code = parts
            .first()
            .map(|c| c.chars().next().unwrap_or('M'))
            .unwrap_or('M');
        let (path, old_path) = match parts.as_slice() {
            [_, old, new] => (new.to_string(), Some(old.to_string())),
            [_, path] => (path.to_string(), None),
            _ => continue,
        };
        let (added, removed) = counts.get(i).copied().unwrap_or((None, None));
        files.push(FileChange {
            path,
            status: code.to_string(),
            old_path,
            added: added.unwrap_or(0),
            removed: removed.unwrap_or(0),
            binary: added.is_none(),
        });
    }
    for path in git(&cwd, &["ls-files", "--others", "--exclude-standard"])?.lines() {
        let lines = std::fs::read(cwd.join(path))
            .ok()
            .filter(|bytes| !bytes.contains(&0))
            .map(|bytes| bytes.iter().filter(|b| **b == b'\n').count() as u64);
        files.push(FileChange {
            path: path.to_string(),
            status: "U".into(),
            old_path: None,
            added: lines.unwrap_or(0),
            removed: 0,
            binary: lines.is_none(),
        });
    }
    let mut diff = git(&cwd, &[&["diff", "--no-color", "-M"], base].concat())?;
    if diff.len() > MAX_DIFF_BYTES {
        let mut cut = MAX_DIFF_BYTES;
        while !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
    }
    Ok(GitChanges {
        branch,
        files,
        diff,
    })
}

/// Opens a Terminal window in an existing folder (a worktree's, from its
/// thread menu).
#[tauri::command(async)]
pub fn open_terminal_at(path: String) -> Result<(), String> {
    let path = absolute_dir(&path)?;
    #[cfg(target_os = "macos")]
    {
        crate::process::command("open")
            .args(["-a", "Terminal"])
            .arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(target_os = "windows")]
    {
        for terminal in ["wt.exe", "cmd.exe"] {
            if interactive_command(terminal)
                .current_dir(&path)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        Err("Could not open Windows Terminal or Command Prompt.".into())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        for terminal in ["x-terminal-emulator", "gnome-terminal", "konsole"] {
            if crate::process::command(terminal)
                .current_dir(&path)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        Err("Could not open a supported terminal.".into())
    }
}

/// Whether the system's Reduce transparency setting is on, which the web
/// view does not report through `prefers-reduced-transparency`.
#[tauri::command(async)]
pub fn reduce_transparency() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::process::command("defaults")
            .args(["read", "com.apple.universalaccess", "reduceTransparency"])
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "1")
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The engine refuses larger images (`MAX_ENCODED_IMAGE_LEN`).
const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// An image file the composer attaches, as an ACP image block takes it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageFile {
    pub name: String,
    pub mime_type: String,
    pub data: String,
}

/// The MIME type the engine accepts for an image path, if any.
pub fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    })
}

/// Read a dropped or picked image for the composer: an absolute path to a
/// PNG, JPEG, GIF or WebP file of at most 20 MB, nothing else.
#[tauri::command(async)]
pub fn read_image(path: String) -> Result<ImageFile, String> {
    use base64::Engine as _;
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err("Not an absolute path.".into());
    }
    let mime = image_mime(&path).ok_or("Only PNG, JPEG, GIF and WebP images attach as images.")?;
    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a file.".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err("Images must be 20 MB or smaller.".into());
    }
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    Ok(ImageFile {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        mime_type: mime.into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// The picture on the system clipboard, as a PNG the composer attaches.
///
/// A web view usually hands a paste event the picture it pasted, but WebKitGTK
/// on Linux can leave `clipboardData` empty and keep a copied screenshot only on
/// the native clipboard. Null when the clipboard holds no picture, so an
/// ordinary Ctrl+V stays an ordinary Ctrl+V.
#[tauri::command(async)]
pub fn clipboard_image() -> Result<Option<ImageFile>, String> {
    use arboard::Error;
    use base64::Engine as _;

    let mut clipboard = match arboard::Clipboard::new() {
        Ok(clipboard) => clipboard,
        // No clipboard to read here at all (a Wayland session without its data
        // control protocol, a headless one): there is nothing to paste.
        Err(Error::ClipboardNotSupported) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let image = match clipboard.get_image() {
        Ok(image) => image,
        Err(Error::ContentNotAvailable | Error::ClipboardNotSupported) => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let png = rgba_as_png(image.width, image.height, &image.bytes)?;
    if png.len() as u64 > MAX_IMAGE_BYTES {
        return Err("Images must be 20 MB or smaller.".into());
    }
    Ok(Some(ImageFile {
        name: "Pasted image".into(),
        mime_type: "image/png".into(),
        data: base64::engine::general_purpose::STANDARD.encode(png),
    }))
}

/// Raw RGBA pixels (what a clipboard hands over) as a PNG, which is what an
/// attachment carries.
fn rgba_as_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 || rgba.is_empty() {
        return Err("The clipboard's image is empty.".into());
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(rgba).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// The MIME type of a picture a reply may show (an attachable image, or SVG).
fn display_mime(path: &Path) -> Option<&'static str> {
    image_mime(path).or_else(|| {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        (ext == "svg").then_some("image/svg+xml")
    })
}

/// A picture a reply's Markdown shows (`![](path)`): an absolute path, or one
/// relative to the thread's folder, that resolves (symlinks followed) inside
/// one of `roots` (the thread's folder and its worktree). Anything else is
/// refused, so a reply cannot read files from elsewhere on the disk.
#[tauri::command(async)]
pub fn thread_image(path: String, roots: Vec<String>) -> Result<ImageFile, String> {
    use base64::Engine as _;
    let roots: Vec<PathBuf> = roots
        .iter()
        .map(PathBuf::from)
        .filter(|r| r.is_absolute())
        .filter_map(|r| r.canonicalize().ok())
        .collect();
    let first = roots.first().ok_or("This thread has no folder.")?;
    let raw = expand_home(&path);
    let joined = if raw.is_absolute() {
        raw
    } else {
        first.join(raw)
    };
    let real = joined
        .canonicalize()
        .map_err(|_| "No such picture.".to_string())?;
    if !roots.iter().any(|root| real.starts_with(root)) {
        return Err("Pictures outside the thread's folder are not shown.".into());
    }
    let mime = display_mime(&real).ok_or("Not a picture.")?;
    let meta = std::fs::metadata(&real).map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Not a file.".into());
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err("Pictures must be 20 MB or smaller.".into());
    }
    let bytes = std::fs::read(&real).map_err(|e| e.to_string())?;
    Ok(ImageFile {
        name: real
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        mime_type: mime.into(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// `~/…` from the engine or the mock, against `$HOME`.
fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// Text files the app may open for editing: nothing that could run.
const EDITABLE: &[&str] = &["md", "markdown", "toml", "json", "txt", "yaml", "yml"];

/// What `open_path` would open: an existing folder, or an existing text file.
fn open_target(path: &str) -> Result<(PathBuf, bool), String> {
    let path = expand_home(path);
    if !path.is_absolute() || !path.exists() {
        return Err(format!("No such file: {}", path.display()));
    }
    if path.is_dir() {
        return Ok((path, true));
    }
    let editable = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EDITABLE.contains(&e.to_ascii_lowercase().as_str()));
    if !editable {
        return Err("Only text files (Markdown, TOML, JSON, YAML) open from here.".into());
    }
    Ok((path, false))
}

/// Opens a skill's SKILL.md, a config or theme file in its editor, or a folder
/// in the file manager. Anything else is refused, so nothing here runs a program.
#[tauri::command(async)]
pub fn open_path(path: String) -> Result<(), String> {
    let (path, dir) = open_target(&path)?;
    open_checked(&path, dir)
}

/// Opens a path the caller already checked: a folder in the file manager, a
/// text file in its editor (Markdown in its own app on macOS).
pub(crate) fn open_checked(path: &Path, dir: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let markdown = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        let mut command = crate::process::command("open");
        // Markdown opens in its own app; other text in the default text editor.
        if !dir && !markdown {
            command.arg("-t");
        }
        command
            .arg(path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = dir;
        open_with_system(&path.display().to_string())
    }
}

const SKILL_TEMPLATE: &str = "---\nname: {name}\ndescription: What this skill does and when the agent should use it, in one sentence.\n---\n\n# {name}\n\nWhat the agent should know or do, step by step.\n\n## When to use\n\n- …\n\n## Steps\n\n1. …\n";

fn valid_skill_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Where a new skill goes: the user's `<config>/skills`, or the project's `.rustcode/skills`.
fn skill_root(scope: &str, cwd: &str, config: &Path) -> Result<PathBuf, String> {
    match scope {
        "user" => Ok(config.join("skills")),
        "project" => Ok(absolute_dir(cwd)?.join(".rustcode/skills")),
        _ => Err("scope must be user or project".into()),
    }
}

fn config_dir() -> Result<PathBuf, String> {
    crate::diagnostics::config_dir().ok_or_else(|| "No home folder.".into())
}

fn write_skill(root: &Path, name: &str) -> Result<PathBuf, String> {
    if !valid_skill_name(name) {
        return Err("Use lowercase letters, digits and dashes (at most 64).".into());
    }
    let dir = root.join(name);
    if dir.exists() {
        return Err(format!("A skill named {name} already exists there."));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = dir.join("SKILL.md");
    std::fs::write(&file, SKILL_TEMPLATE.replace("{name}", name)).map_err(|e| e.to_string())?;
    Ok(file)
}

/// Creates `<root>/<name>/SKILL.md` from the template and answers its path.
#[tauri::command(async)]
pub fn skill_create(name: String, scope: String, cwd: String) -> Result<String, String> {
    let root = skill_root(&scope, &cwd, &config_dir()?)?;
    write_skill(&root, &name).map(|file| file.display().to_string())
}

/// Editors that take `path:line:col`, by their command-line tool, where a
/// GUI app finds them (its PATH is minimal).
const EDITOR_TOOLS: &[(&str, &[&str])] = &[
    (
        "code",
        &[
            "/opt/homebrew/bin/code",
            "/usr/local/bin/code",
            "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        ],
    ),
    (
        "cursor",
        &[
            "/opt/homebrew/bin/cursor",
            "/usr/local/bin/cursor",
            "/Applications/Cursor.app/Contents/Resources/app/bin/cursor",
        ],
    ),
    ("zed", &["/opt/homebrew/bin/zed", "/usr/local/bin/zed"]),
];

/// The file a reference in a reply names: an existing regular file, by absolute path.
fn editor_target(path: &str) -> Result<PathBuf, String> {
    let path = expand_home(path);
    if !path.is_absolute() || !path.is_file() {
        return Err(format!("No such file: {}", path.display()));
    }
    Ok(path)
}

/// Opens a file a reply names, at its line when an editor that takes one is
/// installed (VS Code, Cursor, Zed), else in the default text editor. It
/// only ever opens the file for editing; nothing runs it.
#[tauri::command(async)]
pub fn open_in_editor(path: String, line: Option<u32>, column: Option<u32>) -> Result<(), String> {
    let path = editor_target(&path)?;
    let at = match (line, column) {
        (Some(l), Some(c)) => format!("{}:{l}:{c}", path.display()),
        (Some(l), None) => format!("{}:{l}", path.display()),
        _ => path.display().to_string(),
    };
    for (tool, places) in EDITOR_TOOLS {
        if let Some(bin) = places.iter().map(Path::new).find(|p| p.is_file()) {
            let mut command = crate::process::command(bin);
            if *tool != "zed" {
                command.arg("-g");
            }
            return command
                .arg(&at)
                .spawn()
                .map(|_| ())
                .map_err(|e| e.to_string());
        }
    }
    #[cfg(target_os = "macos")]
    {
        crate::process::command("open")
            .arg("-t")
            .arg(&path)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        open_with_system(&path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_files_lists_what_git_sees() {
        let dir = std::env::temp_dir().join(format!("rc-project-files-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            assert!(crate::process::command("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .status()
                .unwrap()
                .success());
        };
        run(&["init", "-q"]);
        std::fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.join("ignored.txt"), "").unwrap();
        std::fs::write(dir.join("kept.txt"), "").unwrap();
        let files = super::project_files(dir.to_string_lossy().into_owned()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(files.contains(&"kept.txt".to_owned()));
        assert!(!files.contains(&"ignored.txt".to_owned()));
    }

    use super::*;

    #[test]
    fn opens_only_existing_files_in_the_editor() {
        assert!(editor_target("relative.rs").is_err());
        assert!(editor_target("/no/such/file.rs").is_err());
        assert!(editor_target("/tmp").is_err(), "a folder is not a file");
        let file = std::env::temp_dir().join(format!("rc-editor-{}.rs", std::process::id()));
        std::fs::write(&file, "x").unwrap();
        assert_eq!(editor_target(file.to_str().unwrap()).unwrap(), file);
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn opens_only_folders_and_text_files() {
        let dir = std::env::temp_dir().join(format!("rc-open-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("SKILL.md");
        let script = dir.join("run.sh");
        std::fs::write(&md, "x").unwrap();
        std::fs::write(&script, "x").unwrap();
        assert!(open_target(dir.to_str().unwrap()).unwrap().1);
        assert!(!open_target(md.to_str().unwrap()).unwrap().1);
        assert!(open_target(script.to_str().unwrap()).is_err());
        assert!(open_target("relative/SKILL.md").is_err());
        assert!(open_target("/no/such/file.md").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn creates_skills_from_the_template() {
        let root = std::env::temp_dir().join(format!("rc-skill-{}", std::process::id()));
        let file = write_skill(&root, "deploy-app").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("---\nname: deploy-app\n"));
        assert!(
            write_skill(&root, "deploy-app").is_err(),
            "never overwrites"
        );
        for bad in ["", "Deploy", "../x", "a/b", "-x", &"a".repeat(65)] {
            assert!(write_skill(&root, bad).is_err(), "{bad}");
        }
        assert!(skill_root("elsewhere", "/", &root).is_err());
        assert!(skill_root("project", "relative", &root).is_err());
        assert_eq!(skill_root("user", "", &root).unwrap(), root.join("skills"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reads_only_images() {
        assert!(read_image("relative.png".into()).is_err());
        assert!(read_image("/etc/passwd".into()).is_err());
        assert_eq!(image_mime(Path::new("/a/b.JPG")), Some("image/jpeg"));
        assert_eq!(image_mime(Path::new("/a/b.txt")), None);
    }

    #[test]
    fn shows_only_pictures_inside_the_thread_folder() {
        let root = std::env::temp_dir().join(format!("rc-thread-image-{}", std::process::id()));
        let inside = root.join("repo");
        std::fs::create_dir_all(inside.join("docs")).unwrap();
        std::fs::write(inside.join("docs/shot.png"), b"png").unwrap();
        std::fs::write(root.join("secret.png"), b"png").unwrap();
        std::fs::write(inside.join("notes.txt"), b"text").unwrap();
        let roots = vec![inside.to_string_lossy().into_owned()];
        let ok = thread_image("docs/shot.png".into(), roots.clone()).unwrap();
        assert_eq!(ok.mime_type, "image/png");
        let abs = inside.join("docs/shot.png").to_string_lossy().into_owned();
        assert!(thread_image(abs, roots.clone()).is_ok());
        assert!(thread_image("../secret.png".into(), roots.clone()).is_err());
        let outside = root.join("secret.png").to_string_lossy().into_owned();
        assert!(thread_image(outside, roots.clone()).is_err());
        assert!(thread_image("notes.txt".into(), roots.clone()).is_err());
        assert!(thread_image("docs/shot.png".into(), vec!["relative".into()]).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("secret.png"), inside.join("link.png")).unwrap();
            assert!(thread_image("link.png".into(), roots).is_err());
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn quotes_shell_words() {
        assert_eq!(shell_quote("codex"), "codex");
        assert_eq!(shell_quote("/login"), "/login");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn refuses_unknown_login_tools() {
        assert!(open_login_terminal(vec!["rm".into(), "-rf".into()], None).is_err());
        assert!(open_login_terminal(vec!["/tmp/codex".into()], None).is_err());
        assert!(open_login_terminal(vec![], None).is_err());
    }

    #[test]
    fn login_tool_allowlist_rejects_paths() {
        for tool in LOGIN_TOOLS {
            assert!(is_known_login_tool(tool));
        }
        assert!(!is_known_login_tool("C:\\tools\\codex"));
        assert!(!is_known_login_tool("/tmp/codex"));
        assert!(!is_known_login_tool("codex.exe"));
    }

    #[test]
    fn reveal_uses_the_requested_directory_and_file_parent() {
        let directory = std::env::current_dir().unwrap();
        let file = std::env::current_exe().unwrap();
        assert_eq!(reveal_target(&directory, false), directory.as_path());
        assert_eq!(reveal_target(&file, false), file.parent().unwrap());
        assert_eq!(reveal_target(&file, true), file.as_path());
    }

    #[test]
    fn only_a_login_directory_reaches_the_command_line() {
        // The account's directory is validated and quoted, never spliced in raw.
        assert_eq!(
            login_line(
                &["claude".into(), "auth".into(), "login".into()],
                &[("CLAUDE_CONFIG_DIR".into(), "/Users/me/.claude-work".into())],
            ),
            "CLAUDE_CONFIG_DIR=/Users/me/.claude-work claude auth login"
        );
        assert_eq!(
            login_line(
                &["claude".into(), "auth".into(), "login".into()],
                &[("CLAUDE_CONFIG_DIR".into(), "/Users/me/my login".into())],
            ),
            "CLAUDE_CONFIG_DIR='/Users/me/my login' claude auth login"
        );
        assert!(open_login_terminal(
            vec!["claude".into()],
            Some(vec![("PATH".into(), "/tmp".into())])
        )
        .is_err());
    }

    #[test]
    fn a_codex_account_signs_in_with_its_own_home() {
        // `codex login` is a known login tool, and CODEX_HOME its directory.
        assert_eq!(
            login_line(
                &["codex".into(), "login".into()],
                &[("CODEX_HOME".into(), "/Users/me/.codex-paral".into())],
            ),
            "CODEX_HOME=/Users/me/.codex-paral codex login"
        );
        assert_eq!(
            login_line(
                &["codex".into(), "login".into()],
                &[("CODEX_HOME".into(), "/Users/me/my codex".into())],
            ),
            "CODEX_HOME='/Users/me/my codex' codex login"
        );
        assert!(LOGIN_TOOLS.contains(&"codex"));
        assert!(LOGIN_ENV.contains(&"CODEX_HOME"));
        // Another variable, or a tool that is not a harness, is still refused.
        assert!(open_login_terminal(
            vec!["codex".into(), "login".into()],
            Some(vec![("OPENAI_API_KEY".into(), "sk-test".into())])
        )
        .is_err());
        assert!(open_login_terminal(vec!["sh".into()], None).is_err());
    }

    #[test]
    fn checks_branch_bases() {
        assert!(valid_base("main"));
        assert!(valid_base("origin/main"));
        assert!(valid_base("abc123"));
        assert!(valid_base("HEAD~2"));
        assert!(!valid_base("--output=/tmp/x"));
        assert!(!valid_base("a..b"));
        assert!(!valid_base("main; rm"));
        assert!(!valid_base(""));
    }

    #[test]
    fn refuses_terminal_outside_folders() {
        assert!(open_terminal_at("relative".into()).is_err());
        assert!(open_terminal_at("/no/such/folder/here".into()).is_err());
    }

    #[test]
    fn refuses_non_web_links() {
        assert!(open_url("file:///etc/passwd".into()).is_err());
        assert!(open_url("javascript:alert(1)".into()).is_err());
    }

    #[test]
    fn clipboard_pixels_become_a_png() {
        // Two pixels: opaque red and transparent blue, straight through the encoder.
        let rgba = [255, 0, 0, 255, 0, 0, 255, 0];
        let png = rgba_as_png(2, 1, &rgba).expect("a PNG");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

        let mut reader = png::Decoder::new(std::io::Cursor::new(&png[..]))
            .read_info()
            .expect("a readable PNG");
        assert_eq!(reader.info().width, 2);
        assert_eq!(reader.info().height, 1);
        let mut out = vec![0; reader.output_buffer_size().expect("a buffer")];
        let frame = reader.next_frame(&mut out).expect("one frame");
        assert_eq!(&out[..frame.buffer_size()], &rgba);
    }

    #[test]
    fn refuses_an_empty_clipboard_image() {
        assert!(rgba_as_png(0, 0, &[]).is_err());
        assert!(rgba_as_png(4, 4, &[]).is_err());
    }
}

/// A trackpad haptic (NSHapticFeedbackManager): `generic`, `alignment` or
/// `level`. Felt only on a Force Touch trackpad with a finger on it; a no-op
/// elsewhere and off macOS.
#[tauri::command]
pub fn haptic(app: tauri::AppHandle, kind: String) -> Result<(), String> {
    // NSHapticFeedbackPattern: generic 0, alignment 1, levelChange 2.
    let pattern: isize = match kind.as_str() {
        "generic" => 0,
        "alignment" => 1,
        "level" => 2,
        _ => return Err(format!("unknown haptic: {kind}")),
    };
    #[cfg(target_os = "macos")]
    app.run_on_main_thread(move || perform_haptic(pattern))
        .map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "macos"))]
    let _ = (app, pattern);
    Ok(())
}

#[cfg(target_os = "macos")]
fn perform_haptic(pattern: isize) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};
    let Some(manager) = AnyClass::get(c"NSHapticFeedbackManager") else {
        return;
    };
    // SAFETY: documented AppKit class methods, called on the main thread;
    // `defaultPerformer` is never nil, and 1 is NSHapticFeedbackPerformanceTimeNow.
    unsafe {
        let performer: *mut AnyObject = msg_send![manager, defaultPerformer];
        if !performer.is_null() {
            let _: () =
                msg_send![performer, performFeedbackPattern: pattern, performanceTime: 1usize];
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod haptic_tests {
    #[test]
    fn haptic_message_matches_appkit() {
        // objc2 checks argument encodings in debug builds: a wrong type panics here.
        for pattern in 0..3 {
            super::perform_haptic(pattern);
        }
    }
}
