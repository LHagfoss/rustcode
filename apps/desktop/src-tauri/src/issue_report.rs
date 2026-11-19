//! File desktop feedback in the RustCode repository using the user's GitHub CLI.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const REPOSITORY: &str = "repos/lhagfoss/rustcode";

async fn github(args: &[&str], body: Option<Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut path = std::env::var_os("PATH").unwrap_or_default();
    if cfg!(target_os = "macos") {
        path.push(":/opt/homebrew/bin:/usr/local/bin");
    }
    let mut child = Command::from(crate::process::command("gh"));
    let child = child
        .args(args)
        .env("PATH", path)
        .env("GH_PROMPT_DISABLED", "1")
        .env_remove("GH_DEBUG")
        .kill_on_drop(true)
        .stdin(if body.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                "GitHub CLI was not found. Install it from cli.github.com, then run gh auth login."
                    .into()
            } else {
                format!("Could not start GitHub CLI: {error}")
            }
        })?;

    let result = tokio::time::timeout(Duration::from_secs(30), async move {
        let mut child = child;
        if let (Some(mut stdin), Some(body)) = (child.stdin.take(), body) {
            stdin.write_all(&body).await?;
        }
        child.wait_with_output().await
    })
    .await
    .map_err(|_| {
        "GitHub did not respond in time. Check the repository before submitting again.".to_owned()
    })?
    .map_err(|error| format!("Could not read the GitHub CLI response: {error}"))?;

    if result.status.success() {
        Ok(result.stdout)
    } else {
        let details = String::from_utf8_lossy(&result.stderr);
        Err(format!(
            "GitHub CLI could not complete the request. {}",
            details.trim().chars().take(600).collect::<String>()
        ))
    }
}

async fn api(endpoint: &str, args: &[&str], body: Option<Vec<u8>>) -> Result<Value, String> {
    let mut command = vec!["api", "--hostname", "github.com", endpoint];
    command.extend_from_slice(args);
    let output = github(&command, body).await?;
    serde_json::from_slice(&output).map_err(|_| "GitHub returned an unexpected response.".into())
}

#[derive(Serialize)]
pub(crate) struct IssueOptions {
    labels: Vec<String>,
    can_label: bool,
}

#[tauri::command]
pub(crate) async fn issue_report_options() -> Result<IssueOptions, String> {
    let repo = api(REPOSITORY, &[], None).await?;
    let can_label = repo["permissions"]["push"].as_bool().unwrap_or(false);
    let mut labels = Vec::new();
    if can_label {
        let endpoint = format!("{REPOSITORY}/labels?per_page=100");
        let pages = api(&endpoint, &["--paginate", "--slurp"], None).await?;
        labels.extend(
            pages
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_array)
                .flatten()
                .filter_map(|label| label["name"].as_str().map(str::to_owned)),
        );
    }
    Ok(IssueOptions { labels, can_label })
}

#[derive(Deserialize, Serialize)]
pub(crate) struct IssueReport {
    title: String,
    body: String,
    labels: Vec<String>,
}

#[tauri::command]
pub(crate) async fn issue_report_submit(report: IssueReport) -> Result<String, String> {
    if report.title.trim().is_empty()
        || report.title.chars().count() > 256
        || report.body.trim().is_empty()
        || report.body.len() > 65_536
        || report.labels.len() > 100
        || report.labels.iter().any(|label| label.len() > 200)
    {
        return Err("The issue title, description, or labels are invalid.".into());
    }
    let body = serde_json::to_vec(&report).map_err(|error| error.to_string())?;
    let endpoint = format!("{REPOSITORY}/issues");
    let response = api(&endpoint, &["--method", "POST", "--input", "-"], Some(body)).await?;
    let number = response["number"].as_u64().ok_or(
        "GitHub did not return an issue number. Check the repository before submitting again.",
    )?;
    Ok(format!(
        "https://github.com/lhagfoss/rustcode/issues/{number}"
    ))
}

#[tauri::command(async)]
pub(crate) fn issue_report_save_image(
    app: tauri::AppHandle,
    name: String,
    data: String,
) -> Result<(), String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use tauri_plugin_dialog::DialogExt;

    const MAX_BYTES: usize = 10 * 1024 * 1024;
    if data.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err("This image exceeds 10 MB.".into());
    }
    let bytes = STANDARD.decode(data).map_err(|_| "Invalid image data.")?;
    if bytes.len() > MAX_BYTES {
        return Err("This image exceeds 10 MB.".into());
    }
    let name: String = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(160)
        .collect();
    let name = name.trim_matches([' ', '.']);
    let Some(picked) = app
        .dialog()
        .file()
        .set_file_name(if name.is_empty() {
            "Screenshot.png"
        } else {
            name
        })
        .blocking_save_file()
    else {
        return Ok(());
    };
    let path = picked.into_path().map_err(|error| error.to_string())?;
    std::fs::write(path, bytes).map_err(|error| format!("Could not save the screenshot: {error}"))
}
