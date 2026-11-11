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
        ServerCommands::Restart { when_idle, yes } => restart(*when_idle, *yes).await,
        ServerCommands::LinkPath => link_path().await,
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

async fn restart(when_idle: bool, yes: bool) -> anyhow::Result<i32> {
    let running = service::running_turns().await;
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
