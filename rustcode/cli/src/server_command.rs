//! `rustcode server install|status|restart|uninstall|logs`: the commands
//! that manage the background service (`docs/server.md`). The work is in
//! `rustcode::service::server_service`; this prints, asks and exits.

use crate::cli::ServerCommands;
use rustcode::service::{Installed, logs, server_service as service};
use std::io::{IsTerminal, Write};
use std::time::Duration;

/// Run a management command and exit with its status.
pub async fn run(command: &ServerCommands) -> ! {
    let code = match execute(command).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("rustcode server: {error:#}");
            1
        }
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}

async fn execute(command: &ServerCommands) -> anyhow::Result<i32> {
    match command {
        ServerCommands::Install { force_dev, yes } => install(*force_dev, *yes).await,
        ServerCommands::Status { json } => {
            let report = service::status().await?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print!("{}", report.human());
            }
            Ok(0)
        }
        ServerCommands::Restart {
            when_idle,
            yes,
            detached,
        } => restart(*when_idle, *yes, *detached).await,
        ServerCommands::LinkPath => link_path().await,
        ServerCommands::UpdateCli { json } => update_cli(*json).await,
        ServerCommands::Uninstall { yes } => uninstall(*yes).await,
        ServerCommands::Logs { follow, lines } => show_logs(*follow, *lines).await,
        ServerCommands::Run { .. } | ServerCommands::Connect => {
            unreachable!("run and connect are handled where the server starts")
        }
    }
}

async fn install(force_dev: bool, yes: bool) -> anyhow::Result<i32> {
    let report = service::install(force_dev, yes).await?;
    println!(
        "{} ({}): {}",
        match report.outcome {
            Installed::Started => "Installed the RustCode server as a background service",
            Installed::Unchanged => "The RustCode server service is already installed",
            Installed::ChangedWhileActive => "Updated the RustCode server service",
        },
        report.kind,
        report.program.display()
    );
    match &report.live {
        Some(live) => println!("Running: version {}, pid {}.", live.version, live.pid),
        None => println!("The server is not running yet."),
    }
    println!("It starts when you log in; threads keep running when you close the app.");
    for problem in &report.problems {
        println!("{}: {}", problem.code, problem.message);
    }
    Ok(i32::from(report.live.is_none()))
}

async fn restart(when_idle: bool, yes: bool, detached: bool) -> anyhow::Result<i32> {
    if detached {
        // The helper of a restart started inside the server: let that command
        // end and its thread record the result before the server stops.
        tokio::time::sleep(service::HELPER_DELAY).await;
    }
    // A caller that descends from the server cannot restart it: stopping the
    // server ends the caller before it starts the new one.
    let live = service::live_status().await;
    let inside = live
        .as_ref()
        .is_some_and(|live| service::runs_inside(live.pid));
    let run = service::plan_restart_run(inside, detached);
    let running = live.map(|live| live.running_turns);
    match service::plan_restart(when_idle, yes, running) {
        service::RestartPlan::Now => {}
        service::RestartPlan::Confirm { running } => {
            if !confirm(running, "Restart")? {
                println!(
                    "Not restarted. `rustcode server restart --when-idle` waits for the turns to finish."
                );
                return Ok(1);
            }
        }
        // The turn of a thread inside the server runs until this command
        // ends, so the helper does the waiting.
        service::RestartPlan::WaitForIdle if run == service::RestartRun::Detached => {}
        service::RestartPlan::WaitForIdle => {
            let _pending = service::mark_restart_pending()?;
            let outcome = service::wait_until_idle(
                service::running_turns,
                Duration::from_secs(2),
                2,
                None,
                |running| {
                    if running > 0 {
                        eprintln!(
                            "Waiting for {running} running {} to finish...",
                            if running == 1 { "turn" } else { "turns" }
                        );
                    }
                },
            )
            .await;
            if let service::Idle::TimedOut { running } = outcome {
                anyhow::bail!("{running} turns were still running");
            }
        }
    }
    if run == service::RestartRun::Detached {
        service::spawn_detached_restart(when_idle)?;
        println!(
            "This runs inside the server, so the restart continues in the background{}. \
             See `rustcode server logs` for the result.",
            if when_idle {
                ", once no turn is running"
            } else {
                " after this command ends"
            }
        );
        return Ok(0);
    }
    match service::restart().await? {
        Some(live) => {
            println!("Restarted: version {}, pid {}.", live.version, live.pid);
            Ok(0)
        }
        None => {
            anyhow::bail!("the server did not answer after the restart; see `rustcode server logs`")
        }
    }
}

async fn link_path() -> anyhow::Result<i32> {
    use rustcode::service::path_binary::{Linked, link};
    match link(env!("CARGO_PKG_VERSION")).await? {
        Linked::NotOnPath => {
            println!("No `rustcode` is on PATH, so there is nothing to point at this one.");
            Ok(1)
        }
        Linked::Current(path) => {
            println!(
                "{} already runs this version ({}).",
                path.display(),
                env!("CARGO_PKG_VERSION")
            );
            Ok(0)
        }
        Linked::Homebrew(path) => {
            println!(
                "{} is Homebrew's copy; update it with `brew upgrade lhagfoss/tap/rustcode`.",
                path.display()
            );
            Ok(1)
        }
        Linked::Replaced { path, was } => {
            println!(
                "{} now runs this version ({}){}.",
                path.display(),
                env!("CARGO_PKG_VERSION"),
                was.map(|was| format!(", it was {was}")).unwrap_or_default()
            );
            Ok(0)
        }
    }
}

/// What `update-cli` found and did about one copy.
#[derive(Clone, serde::Serialize)]
struct CopyReport {
    /// `missing`, `current`, `updated`, `homebrew`, `cargo`, `development`,
    /// `foreign` or `failed`.
    outcome: &'static str,
    /// The copy it looked at, when one was there.
    path: Option<String>,
    /// The version the copy reported before it was replaced.
    was: Option<String>,
    /// The version the copy answers now, when it is one the app may restart a
    /// server from: this engine's after a replace, its own when it was left.
    now: Option<String>,
    /// What a person should read.
    message: String,
}

/// What `update-cli` found and did, for a person and for the app: the copy a
/// shell runs first (the top-level fields), then every copy it looked at.
#[derive(serde::Serialize)]
struct UpdatedCli {
    #[serde(flatten)]
    first: CopyReport,
    /// The version this engine is.
    version: &'static str,
    copies: Vec<CopyReport>,
}

/// Bring the `rustcode` copies the app is responsible for up to this engine's
/// version (issue #3487). The app runs this after an update, when its own
/// engine is the new version; a copy that is current, newer or unreadable is
/// left alone, as is one Homebrew or `cargo install` owns, a development
/// build, and any copy this app did not write.
async fn update_cli(json: bool) -> anyhow::Result<i32> {
    use rustcode::service::path_binary::update_installed_copies;
    let version = env!("CARGO_PKG_VERSION");
    let mut copies = Vec::new();
    let mut code = 0;
    for copy in update_installed_copies(version).await? {
        let (report, copy_code) = describe_copy(copy, version);
        code = code.max(copy_code);
        copies.push(report);
    }
    if json {
        // `update_installed_copies` always answers at least one copy.
        let first = copies[0].clone();
        let report = UpdatedCli {
            first,
            version,
            copies,
        };
        println!("{}", serde_json::to_string(&report)?);
    } else {
        for report in &copies {
            println!("{}", report.message);
        }
    }
    Ok(code)
}

fn describe_copy(
    copy: rustcode::service::path_binary::InstalledCopy,
    version: &'static str,
) -> (CopyReport, i32) {
    use rustcode::service::path_binary::InstalledCopy;
    let brew = "`brew upgrade lhagfoss/tap/rustcode`";
    let shown = |path: &std::path::Path| Some(path.display().to_string());
    match copy {
        InstalledCopy::Missing => (
            CopyReport {
                outcome: "missing",
                path: None,
                was: None,
                now: None,
                message: "No `rustcode` is installed where a shell or an install puts one, so there is nothing to update.".to_owned(),
            },
            0,
        ),
        InstalledCopy::Current { path, version: now } => (
            CopyReport {
                outcome: "current",
                path: shown(&path),
                was: None,
                now,
                message: format!("{} is already current ({version}).", path.display()),
            },
            0,
        ),
        InstalledCopy::Replaced { path, was } => {
            let before = was
                .as_ref()
                .map(|was| format!(", it was {was}"))
                .unwrap_or_default();
            (
                CopyReport {
                    outcome: "updated",
                    path: shown(&path),
                    was,
                    now: Some(version.to_owned()),
                    message: format!("{} now runs this version ({version}){before}.", path.display()),
                },
                0,
            )
        }
        InstalledCopy::Managed { path, by: "Homebrew" } => (
            CopyReport {
                outcome: "homebrew",
                path: shown(&path),
                was: None,
                now: None,
                message: format!(
                    "{} is Homebrew's copy (managed by Homebrew); update it with {brew}.",
                    path.display()
                ),
            },
            1,
        ),
        InstalledCopy::Managed { path, by } => (
            CopyReport {
                outcome: "cargo",
                path: shown(&path),
                was: None,
                now: None,
                message: format!(
                    "{} is `cargo install`'s copy (managed by {by}); update it with `cargo install --path rustcode/tui`, or remove it.",
                    path.display()
                ),
            },
            1,
        ),
        InstalledCopy::Development(path) => (
            CopyReport {
                outcome: "development",
                path: shown(&path),
                was: None,
                now: None,
                message: format!(
                    "{} is a development build (inside a cargo target directory); a released RustCode does not replace it.",
                    path.display()
                ),
            },
            1,
        ),
        InstalledCopy::Foreign(path) => (
            CopyReport {
                outcome: "foreign",
                path: shown(&path),
                was: None,
                now: None,
                message: format!(
                    "{} is a copy this app did not install, so it is left as it is; update it yourself, or remove it so a shell finds this app's version.",
                    path.display()
                ),
            },
            1,
        ),
        InstalledCopy::Failed { path, error } => (
            CopyReport {
                outcome: "failed",
                path: shown(&path),
                was: None,
                now: None,
                message: format!("{} could not be updated: {error}", path.display()),
            },
            1,
        ),
    }
}

async fn uninstall(yes: bool) -> anyhow::Result<i32> {
    if let Some(running) = service::running_turns().await
        && running > 0
        && !yes
        && !confirm(running, "Uninstall")?
    {
        println!("Not uninstalled.");
        return Ok(1);
    }
    if service::uninstall()? {
        println!("Removed the RustCode server service. Threads, settings and pairings are kept.");
    } else {
        println!("The RustCode server service was not installed.");
    }
    Ok(0)
}

/// Ask whether to end `running` turns; with no terminal to ask on, refuse.
fn confirm(running: usize, action: &str) -> anyhow::Result<bool> {
    let turns = format!(
        "{running} {} running",
        if running == 1 { "turn is" } else { "turns are" }
    );
    anyhow::ensure!(
        std::io::stdin().is_terminal(),
        "{turns}, and this stops the server and ends them; pass --yes to do it anyway"
    );
    print!("{turns}, and this stops the server and ends them. {action} anyway? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

async fn show_logs(follow: bool, lines: usize) -> anyhow::Result<i32> {
    let path = rustcode::server::log_path()?;
    let (text, end) = logs::tail(&path, lines)?;
    if text.is_empty() && !follow {
        println!("No server log yet at {}.", path.display());
        return Ok(0);
    }
    let mut stdout = std::io::stdout();
    stdout.write_all(text.as_bytes())?;
    stdout.flush()?;
    if !follow {
        return Ok(0);
    }
    let mut follower = logs::Follower::new(&path, end);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let new = follower.read_new()?;
        if !new.is_empty() {
            // A closed pipe (`| head`) ends the command.
            if stdout
                .write_all(new.as_bytes())
                .and_then(|()| stdout.flush())
                .is_err()
            {
                return Ok(0);
            }
        }
    }
}
