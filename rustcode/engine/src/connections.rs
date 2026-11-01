//! Accounts RustCode works through, for the macOS Settings "Connections" page:
//! GitHub through the GitHub CLI (`gh`) and the saved model accounts.
//!
//! **GitHub.** RustCode keeps no GitHub token of its own. `gh` already stores
//! one in the system keychain and the agent's commands use it, so the page
//! reports `gh auth status` and signs in by running `gh auth login --web`: the
//! one-time code it prints is shown in the app, and the sign-in finishes when
//! `gh` exits after the browser step. Signing out runs `gh auth logout`.
//!
//! **Model accounts.** Listed from the saved account store; signing one out is
//! the same as `/logout <provider> <account>`.

use serde::Serialize;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

pub const GITHUB_HOST: &str = "github.com";
/// How long `gh` may take to print its one-time code.
const CODE_TIMEOUT: Duration = Duration::from_secs(20);
/// Lines of `gh` output kept to explain a failed sign-in.
const KEPT_ERROR_LINES: usize = 6;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionsSnapshot {
    pub github: GitHubStatus,
    pub accounts: Vec<ModelAccount>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubStatus {
    /// Whether the GitHub CLI was found.
    pub cli_installed: bool,
    pub accounts: Vec<GitHubAccount>,
    /// A sign-in waiting for the browser step.
    pub pending: Option<DeviceCode>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubAccount {
    pub host: String,
    pub login: String,
    pub active: bool,
    /// False when `gh` has the account but its token no longer works.
    pub valid: bool,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub code: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAccount {
    pub provider: String,
    pub account: String,
    pub display: String,
    /// `api_key`, `chat_gpt`, `github_copilot` or `claude_cli`.
    pub method: crate::provider_auth::AuthMethod,
    pub active: bool,
    pub expires_at: Option<u64>,
    /// The saved sign-in was refused; `/login` again (#2765).
    pub needs_sign_in: bool,
}

pub async fn snapshot() -> ConnectionsSnapshot {
    let accounts = crate::provider_auth::saved_accounts()
        .unwrap_or_default()
        .into_iter()
        .map(|row| ModelAccount {
            provider: row.provider,
            account: row.account,
            display: row.display,
            method: row.method,
            active: row.active,
            expires_at: row.expires_at,
            needs_sign_in: row.needs_sign_in,
        })
        .collect();
    ConnectionsSnapshot {
        github: github_status().await,
        accounts,
    }
}

fn gh() -> std::path::PathBuf {
    crate::platform::resolve_bin("gh")
}

async fn github_status() -> GitHubStatus {
    let pending = LOGIN
        .lock()
        .expect("login mutex")
        .as_ref()
        .and_then(|login| login.done.borrow().is_none().then(|| login.code.clone()));
    let output = tokio::process::Command::new(gh())
        .args(["auth", "status", "--json", "hosts"])
        .stdin(Stdio::null())
        .output()
        .await;
    let output = match output {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return GitHubStatus {
                pending,
                ..GitHubStatus::default()
            };
        }
        Err(error) => {
            return GitHubStatus {
                cli_installed: true,
                pending,
                error: Some(format!("Could not run gh: {error}")),
                ..GitHubStatus::default()
            };
        }
    };
    // `gh auth status` exits non-zero when any host fails, but the JSON still
    // lists every account.
    match parse_status(&String::from_utf8_lossy(&output.stdout)) {
        Some(accounts) => GitHubStatus {
            cli_installed: true,
            accounts,
            pending,
            error: None,
        },
        None => GitHubStatus {
            cli_installed: true,
            pending,
            error: Some(last_lines(&String::from_utf8_lossy(&output.stderr))),
            ..GitHubStatus::default()
        },
    }
}

/// Accounts from `gh auth status --json hosts`.
fn parse_status(json: &str) -> Option<Vec<GitHubAccount>> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let hosts = value.get("hosts")?.as_object()?;
    let mut accounts = Vec::new();
    for (host, entries) in hosts {
        for entry in entries.as_array().into_iter().flatten() {
            let text = |key: &str| entry.get(key).and_then(|v| v.as_str()).unwrap_or_default();
            accounts.push(GitHubAccount {
                host: host.clone(),
                login: text("login").to_owned(),
                active: entry
                    .get("active")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                valid: text("state") == "success",
                scopes: text("scopes")
                    .split(',')
                    .map(str::trim)
                    .filter(|scope| !scope.is_empty())
                    .map(str::to_owned)
                    .collect(),
            });
        }
    }
    accounts.sort_by(|a, b| b.active.cmp(&a.active).then(a.login.cmp(&b.login)));
    Some(accounts)
}

/// The one-time code or the device URL from a line `gh auth login --web`
/// prints.
fn parse_login_line(line: &str, code: &mut Option<String>, url: &mut Option<String>) {
    if let Some((_, rest)) = line.split_once("one-time code:") {
        let value = rest.trim();
        if !value.is_empty() {
            *code = Some(value.to_owned());
        }
    }
    if let Some(start) = line.find("https://") {
        let value = line[start..].split_whitespace().next().unwrap_or_default();
        *url = Some(value.to_owned());
    }
}

fn last_lines(text: &str) -> String {
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    lines[lines.len().saturating_sub(KEPT_ERROR_LINES)..].join("\n")
}

/// A running `gh auth login --web`.
struct Login {
    code: DeviceCode,
    cancel: CancellationToken,
    done: watch::Receiver<Option<Result<(), String>>>,
}

static LOGIN: Mutex<Option<Login>> = Mutex::new(None);

/// Start signing in to GitHub and return the code to enter in the browser.
/// A sign-in already running is cancelled first.
pub async fn start_github_login() -> Result<DeviceCode, String> {
    cancel_github_login();
    let mut child = tokio::process::Command::new(gh())
        .args(["auth", "login", "--web", "--hostname", GITHUB_HOST])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => {
                "The GitHub CLI (gh) is not installed. Install it with `brew install gh`."
                    .to_owned()
            }
            _ => format!("Could not run gh: {error}"),
        })?;
    let mut stderr = BufReader::new(child.stderr.take().expect("piped stderr")).lines();
    let stdout = child.stdout.take().expect("piped stdout");
    let kept = Arc::new(Mutex::new(Vec::<String>::new()));
    let (mut code, mut url) = (None, None);
    let found = tokio::time::timeout(CODE_TIMEOUT, async {
        while let Ok(Some(line)) = stderr.next_line().await {
            parse_login_line(&line, &mut code, &mut url);
            keep(&kept, line);
            if code.is_some() && url.is_some() {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    let (Some(code), Some(url)) = (code.filter(|_| found), url) else {
        let _ = child.kill().await;
        let detail = last_lines(&kept.lock().expect("kept lines").join("\n"));
        return Err(if detail.is_empty() {
            "gh did not print a sign-in code.".to_owned()
        } else {
            detail
        });
    };
    let code = DeviceCode { code, url };
    let cancel = CancellationToken::new();
    let (sender, done) = watch::channel(None);
    // Keep reading both pipes so gh never blocks on a full one.
    tokio::spawn({
        let kept = kept.clone();
        async move {
            while let Ok(Some(line)) = stderr.next_line().await {
                keep(&kept, line);
            }
        }
    });
    tokio::spawn({
        let kept = kept.clone();
        async move {
            let mut stdout = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = stdout.next_line().await {
                keep(&kept, line);
            }
        }
    });
    tokio::spawn({
        let cancel = cancel.clone();
        async move {
            let outcome = tokio::select! {
                status = child.wait() => match status {
                    Ok(status) if status.success() => Ok(()),
                    Ok(_) => Err(last_lines(&kept.lock().expect("kept lines").join("\n"))),
                    Err(error) => Err(error.to_string()),
                },
                () = cancel.cancelled() => {
                    let _ = child.kill().await;
                    Err("Sign-in cancelled.".to_owned())
                }
            };
            let _ = sender.send(Some(outcome));
        }
    });
    *LOGIN.lock().expect("login mutex") = Some(Login {
        code: code.clone(),
        cancel,
        done,
    });
    Ok(code)
}

fn keep(kept: &Mutex<Vec<String>>, line: String) {
    let mut kept = kept.lock().expect("kept lines");
    kept.push(line);
    let excess = kept.len().saturating_sub(KEPT_ERROR_LINES * 4);
    kept.drain(..excess);
}

/// Wait for the running sign-in to finish.
pub async fn wait_github_login() -> Result<(), String> {
    let done = LOGIN
        .lock()
        .expect("login mutex")
        .as_ref()
        .map(|login| login.done.clone());
    let Some(mut done) = done else {
        return Err("No GitHub sign-in is running.".to_owned());
    };
    let outcome = done
        .wait_for(Option::is_some)
        .await
        .map_err(|_| "The sign-in stopped.".to_owned())?
        .clone()
        .expect("checked by wait_for");
    outcome
}

pub fn cancel_github_login() {
    if let Some(login) = LOGIN.lock().expect("login mutex").take() {
        login.cancel.cancel();
    }
}

pub async fn github_logout(host: &str, login: &str) -> Result<(), String> {
    let output = tokio::process::Command::new(gh())
        .args(["auth", "logout", "--hostname", host, "--user", login])
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("Could not run gh: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(last_lines(&String::from_utf8_lossy(&output.stderr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_lists_every_account_active_first() {
        let json = r#"{"hosts":{"github.com":[
            {"state":"success","active":false,"host":"github.com","login":"zed","scopes":"repo"},
            {"state":"success","active":true,"host":"github.com","login":"amy","scopes":"admin:public_key, gist, read:org, repo"}
        ],"ghe.example":[{"state":"error","active":true,"host":"ghe.example","login":"amy","scopes":""}]}}"#;
        let accounts = parse_status(json).unwrap();
        assert_eq!(accounts.len(), 3);
        assert!(accounts[0].active);
        let amy = accounts
            .iter()
            .find(|a| a.host == "github.com" && a.login == "amy")
            .unwrap();
        assert_eq!(amy.scopes, ["admin:public_key", "gist", "read:org", "repo"]);
        assert!(amy.valid);
        let enterprise = accounts.iter().find(|a| a.host == "ghe.example").unwrap();
        assert!(!enterprise.valid);
        assert!(enterprise.scopes.is_empty());
        assert_eq!(parse_status(r#"{"hosts":{}}"#), Some(Vec::new()));
        assert_eq!(parse_status("not json"), None);
    }

    #[test]
    fn the_snapshot_wire_shape_matches_the_mac_app() {
        let snapshot = ConnectionsSnapshot {
            github: GitHubStatus {
                cli_installed: true,
                accounts: vec![GitHubAccount {
                    host: "github.com".into(),
                    login: "amy".into(),
                    active: true,
                    valid: true,
                    scopes: vec!["repo".into()],
                }],
                pending: Some(DeviceCode {
                    code: "37B2-1C7B".into(),
                    url: "https://github.com/login/device".into(),
                }),
                error: None,
            },
            accounts: vec![ModelAccount {
                provider: "openai".into(),
                account: "me@example.com".into(),
                display: "Me".into(),
                method: crate::provider_auth::AuthMethod::ChatGpt,
                active: true,
                expires_at: Some(1_900_000_000),
                needs_sign_in: false,
            }],
        };
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            serde_json::json!({
                "github": {
                    "cliInstalled": true,
                    "accounts": [{"host": "github.com", "login": "amy", "active": true, "valid": true, "scopes": ["repo"]}],
                    "pending": {"code": "37B2-1C7B", "url": "https://github.com/login/device"},
                    "error": null,
                },
                "accounts": [{"provider": "openai", "account": "me@example.com", "display": "Me",
                              "method": "chat_gpt", "active": true, "expiresAt": 1_900_000_000u64,
                              "needsSignIn": false}],
            })
        );
    }

    #[test]
    fn login_output_yields_the_code_and_url() {
        let (mut code, mut url) = (None, None);
        for line in [
            "",
            "! First copy your one-time code: 37B2-1C7B",
            "Open this URL to continue in your web browser: https://github.com/login/device",
        ] {
            parse_login_line(line, &mut code, &mut url);
        }
        assert_eq!(code.as_deref(), Some("37B2-1C7B"));
        assert_eq!(url.as_deref(), Some("https://github.com/login/device"));
    }

    #[test]
    fn failures_keep_the_last_lines() {
        let text = (1..=10)
            .map(|n| format!("line {n}\n\n"))
            .collect::<String>();
        assert_eq!(
            last_lines(&text),
            "line 5\nline 6\nline 7\nline 8\nline 9\nline 10"
        );
    }
}
