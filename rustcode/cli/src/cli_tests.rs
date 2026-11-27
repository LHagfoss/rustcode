use super::*;
use clap::CommandFactory;

#[test]
fn parses_update_and_upgrade_flags() {
    let cli = Cli::try_parse_from(["rustcode", "--update"]).unwrap();
    assert!(cli.update);
    let cli_alias = Cli::try_parse_from(["rustcode", "--upgrade"]).unwrap();
    assert!(cli_alias.update);
}

#[test]
fn update_help_describes_supported_install_sources() {
    let help = Cli::command().render_long_help().to_string();
    assert!(help.contains("GitHub Release"));
    assert!(help.contains("Homebrew"));
}

#[test]
fn parses_acp_flag() {
    let cli = Cli::try_parse_from(["rc", "--acp"]).unwrap();
    assert!(cli.acp);
}

#[test]
fn parses_sessions_list_with_all_flag() {
    let threads = Cli::try_parse_from(["rustcode", "threads", "list"]).unwrap();
    assert!(matches!(
        threads.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::List { all: false })
        })
    ));
    let cli = Cli::try_parse_from(["rustcode", "sessions", "list"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::List { all: false })
        })
    ));
    let cli_all = Cli::try_parse_from(["rustcode", "sessions", "list", "--all"]).unwrap();
    assert!(matches!(
        cli_all.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::List { all: true })
        })
    ));
}

#[test]
fn parses_sessions_search() {
    let cli = Cli::try_parse_from(["rustcode", "sessions", "search", "parser"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Search { query, all: false, limit: 50, json: false })
        }) if query == "parser"
    ));
    let cli = Cli::try_parse_from([
        "rustcode",
        "sessions",
        "search",
        "two words",
        "--all",
        "--limit",
        "5",
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Search { query, all: true, limit: 5, json: true })
        }) if query == "two words"
    ));
}

#[test]
fn parses_threads_settle_unsettle_and_review_with_several_ids() {
    let cli = Cli::try_parse_from(["rustcode", "threads", "settle", "a", "b"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Settle { ids })
        }) if ids == ["a".to_owned(), "b".to_owned()]
    ));
    let cli = Cli::try_parse_from(["rustcode", "threads", "unsettle", "a"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Unsettle { ids })
        }) if ids == ["a".to_owned()]
    ));
    let cli = Cli::try_parse_from(["rustcode", "threads", "review", "a"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Review { ids })
        }) if ids == ["a".to_owned()]
    ));
    assert!(Cli::try_parse_from(["rc", "threads", "settle"]).is_err());
}

#[test]
fn parses_bench_stats() {
    let cli = Cli::try_parse_from([
        "rustcode",
        "bench",
        "--rounds",
        "4",
        "--calls",
        "6",
        "--recoveries",
        "1",
        "--completed",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Bench {
            rounds: 4,
            calls: 6,
            recoveries: 1,
            completed: true,
            ..
        })
    ));
}

#[test]
fn parses_theme_commands() {
    let parse = |args: &[&str]| {
        let mut argv = vec!["rustcode", "theme"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv).map(|cli| cli.command) {
            Ok(Some(Commands::Theme { command })) => Some(command),
            _ => None,
        }
    };
    assert!(matches!(parse(&["list"]), Some(ThemeCommands::List)));
    assert!(matches!(
        parse(&["show", "nord", "--json"]),
        Some(ThemeCommands::Show { name, json: true }) if name == "nord"
    ));
    assert!(matches!(
        parse(&["new", "mine", "--from", "light"]),
        Some(ThemeCommands::New { name, from: Some(from) }) if name == "mine" && from == "light"
    ));
    assert!(matches!(
        parse(&["new", "mine"]),
        Some(ThemeCommands::New { from: None, .. })
    ));
    assert!(matches!(
        parse(&["set", "dracula"]),
        Some(ThemeCommands::Set { name }) if name == "dracula"
    ));
    assert!(matches!(
        parse(&["validate"]),
        Some(ThemeCommands::Validate { target: None })
    ));
    assert!(matches!(
        parse(&["validate", "./x.toml"]),
        Some(ThemeCommands::Validate { target: Some(_) })
    ));
    assert!(matches!(
        parse(&["path"]),
        Some(ThemeCommands::Path { name: None })
    ));
    assert!(matches!(
        parse(&["path", "nord"]),
        Some(ThemeCommands::Path { name: Some(_) })
    ));
    assert!(parse(&["show"]).is_none(), "show needs a name");
    assert!(parse(&["paint"]).is_none());
}

#[test]
fn parses_doctor_with_fix_flag() {
    let cli = Cli::try_parse_from(["rustcode", "doctor"]).unwrap();
    assert!(matches!(cli.command, Some(Commands::Doctor { fix: false })));
    let cli_fix = Cli::try_parse_from(["rustcode", "doctor", "--fix"]).unwrap();
    assert!(matches!(
        cli_fix.command,
        Some(Commands::Doctor { fix: true })
    ));
}

#[test]
fn parses_acp_subcommand() {
    assert!(Cli::try_parse_from(["rustcode", "acp"]).is_ok());
}

#[test]
fn parses_yolo_flag() {
    let cli = Cli::try_parse_from(["rustcode", "--yolo"]).unwrap();
    assert!(cli.yolo);
}

#[test]
fn parses_project_init_flag_and_subcommand() {
    let flag = Cli::try_parse_from(["rustcode", "--init"]).unwrap();
    assert!(flag.init);
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "init"]).unwrap().command,
        Some(Commands::Init)
    ));
}

#[test]
fn parses_session_migration_dry_run() {
    let cli = Cli::try_parse_from(["rustcode", "sessions", "migrate", "--dry-run"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Migrate { dry_run: true })
        })
    ));
    let export = Cli::try_parse_from(["rustcode", "export", "abc", "--out", "/tmp"]).unwrap();
    assert!(matches!(
        export.command,
        Some(Commands::Export { id: Some(id), out: Some(_) }) if id == "abc"
    ));
    let export = Cli::try_parse_from(["rustcode", "sessions", "export"]).unwrap();
    assert!(matches!(
        export.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Export {
                id: None,
                out: None
            })
        })
    ));
    let path = Cli::try_parse_from(["rustcode", "sessions", "path", "abc"]).unwrap();
    assert!(matches!(
        path.command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Path { id })
        }) if id == "abc"
    ));
}

#[test]
fn parses_storage_status() {
    let cli = Cli::try_parse_from(["rustcode", "storage", "status", "--json"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Storage {
            command: StorageCommands::Status { json: true }
        })
    ));
}

#[test]
fn parses_storage_clean() {
    let cli = Cli::try_parse_from(["rustcode", "storage", "clean", "--apply"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Storage {
            command: StorageCommands::Clean {
                apply: true,
                json: false
            }
        })
    ));
}

#[test]
fn parses_storage_usage() {
    let cli = Cli::try_parse_from(["rustcode", "storage", "usage"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Storage {
            command: StorageCommands::Usage { json: false }
        })
    ));
}

#[test]
fn parses_discord_configuration_flags() {
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "discord", "--setup"])
            .unwrap()
            .command,
        Some(Commands::Discord { setup: true, .. })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "discord", "--status"])
            .unwrap()
            .command,
        Some(Commands::Discord { status: true, .. })
    ));
    assert!(Cli::try_parse_from(["rustcode", "discord", "--enable"]).is_ok());
    assert!(Cli::try_parse_from(["rustcode", "discord", "--disable"]).is_ok());
}

#[test]
fn discord_configuration_flags_are_mutually_exclusive() {
    assert!(Cli::try_parse_from(["rustcode", "discord", "--enable", "--disable"]).is_err());
}

#[test]
fn parses_sync_direction_flags() {
    let pull = Cli::try_parse_from(["rustcode", "sync", "--pull"]).unwrap();
    assert!(matches!(
        pull.command,
        Some(Commands::Sync {
            pull: true,
            push: false,
            command: None
        })
    ));

    let push = Cli::try_parse_from(["rustcode", "sync", "--push"]).unwrap();
    assert!(matches!(
        push.command,
        Some(Commands::Sync {
            pull: false,
            push: true,
            command: None
        })
    ));
}

#[test]
fn preserves_sync_subcommands_and_init() {
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "sync", "pull"])
            .unwrap()
            .command,
        Some(Commands::Sync {
            pull: false,
            push: false,
            command: Some(SyncCommands::Pull)
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "sync", "push"])
            .unwrap()
            .command,
        Some(Commands::Sync {
            pull: false,
            push: false,
            command: Some(SyncCommands::Push)
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "sync", "init", "origin"])
            .unwrap()
            .command,
        Some(Commands::Sync {
            pull: false,
            push: false,
            command: Some(SyncCommands::Init { remote_url })
        }) if remote_url == "origin"
    ));
}

#[test]
fn rejects_both_sync_direction_flags() {
    let error = Cli::try_parse_from(["rustcode", "sync", "--pull", "--push"])
        .unwrap_err()
        .to_string();
    assert!(error.contains("cannot be used with"));
}

#[test]
fn resolves_sync_dispatch_actions() {
    assert_eq!(
        resolve_sync_action(None, false, false),
        Ok(SyncAction::PullThenPush)
    );
    assert_eq!(resolve_sync_action(None, true, false), Ok(SyncAction::Pull));
    assert_eq!(resolve_sync_action(None, false, true), Ok(SyncAction::Push));
    assert_eq!(
        resolve_sync_action(Some(SyncCommands::Pull), false, false),
        Ok(SyncAction::Pull)
    );
    assert_eq!(
        resolve_sync_action(Some(SyncCommands::Push), false, false),
        Ok(SyncAction::Push)
    );
    assert_eq!(
        resolve_sync_action(
            Some(SyncCommands::Init {
                remote_url: "origin".to_string()
            }),
            false,
            false
        ),
        Ok(SyncAction::Init("origin".to_string()))
    );
    assert_eq!(
        resolve_sync_action(None, true, true),
        Err("--pull and --push cannot be used together")
    );
    assert_eq!(
        resolve_sync_action(Some(SyncCommands::Pull), true, false),
        Err("sync flags cannot be combined with a sync subcommand")
    );
}

#[test]
fn parses_remote_gateway_commands() {
    let cli = Cli::try_parse_from(["rustcode", "remote", "serve"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Remote {
            command: RemoteCommands::Serve {
                ref bind,
                port: 17879,
                advertise: None,
                prefer_interface: None,
            }
        }) if bind == "auto"
    ));
    let cli = Cli::try_parse_from([
        "rustcode",
        "remote",
        "serve",
        "--bind",
        "0.0.0.0",
        "--port",
        "9000",
        "--advertise",
        "100.64.0.7",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Remote {
            command: RemoteCommands::Serve {
                port: 9000,
                advertise: Some(ref advertise),
                ..
            }
        }) if advertise == "100.64.0.7"
    ));
    for action in ["pair", "devices", "status", "stop"] {
        let cli = Cli::try_parse_from(["rustcode", "remote", action]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Remote { .. })));
    }
    for (alias, want) in [
        ("on", "Start"),
        ("off", "Stop"),
        ("start", "Start"),
        ("stop", "Stop"),
    ] {
        let cli = Cli::try_parse_from(["rustcode", "remote", alias]).unwrap();
        assert!(
            matches!(&cli.command, Some(Commands::Remote { command }) if format!("{command:?}").starts_with(want)),
            "unexpected parse for remote {alias}: {cli:?}"
        );
    }
    let cli = Cli::try_parse_from(["rustcode", "remote", "revoke", "phone"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Remote {
            command: RemoteCommands::Revoke { ref device }
        }) if device == "phone"
    ));
    assert!(Cli::try_parse_from(["rustcode", "remote", "revoke"]).is_err());
    assert!(Cli::try_parse_from(["rustcode", "remote"]).is_err());
}

#[test]
fn parses_up() {
    let up = |args: &[&str]| {
        let mut all = vec!["rustcode", "up"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).unwrap().command
    };
    assert!(matches!(
        up(&[]),
        Some(Commands::Up {
            yes: false,
            json: false,
            no_service: false
        })
    ));
    assert!(matches!(
        up(&["--yes", "--json", "--no-service"]),
        Some(Commands::Up {
            yes: true,
            json: true,
            no_service: true
        })
    ));
}

#[test]
fn parses_machine_commands() {
    let machines = |args: &[&str]| {
        let mut all = vec!["rustcode", "machines"];
        all.extend_from_slice(args);
        match Cli::try_parse_from(all).map(|cli| cli.command) {
            Ok(Some(Commands::Machines { command })) => Ok(command),
            Ok(other) => panic!("expected a machines command, got {other:?}"),
            Err(error) => Err(error.to_string()),
        }
    };
    assert!(matches!(
        machines(&["list"]).unwrap(),
        MachineCommands::List {
            threads: false,
            json: false
        }
    ));
    match machines(&["add-ssh", "lars@box", "-p", "2222", "-i", "/k/id", "--yes"]).unwrap() {
        MachineCommands::AddSsh {
            target,
            port,
            identity,
            yes,
            no_install,
            mutual,
            one_way,
            json,
            name,
        } => {
            assert!(!mutual && !one_way);
            assert_eq!(target, "lars@box");
            assert_eq!(port, Some(2222));
            assert_eq!(identity, Some(std::path::PathBuf::from("/k/id")));
            assert!(yes && !no_install && !json);
            assert_eq!(name, None);
        }
        other => panic!("expected add-ssh, got {other:?}"),
    }
    assert!(
        machines(&["add-ssh", "box", "--yes", "--no-install"]).is_err(),
        "installing without asking and never installing conflict"
    );
    assert!(matches!(
        machines(&["add-ssh", "box", "--mutual"]).unwrap(),
        MachineCommands::AddSsh {
            mutual: true,
            one_way: false,
            ..
        }
    ));
    assert!(matches!(
        machines(&["add-ssh", "box", "--one-way"]).unwrap(),
        MachineCommands::AddSsh {
            mutual: false,
            one_way: true,
            ..
        }
    ));
    assert!(
        machines(&["add-ssh", "box", "--mutual", "--one-way"]).is_err(),
        "both ways and one way conflict"
    );
    assert!(matches!(
        machines(&["list", "--threads", "--json"]).unwrap(),
        MachineCommands::List {
            threads: true,
            json: true
        }
    ));
    assert!(matches!(
        machines(&["add", "192.168.1.20:17879", "--name", "Studio", "--code", "4821-9034"])
            .unwrap(),
        MachineCommands::Add {
            ref address,
            ref name,
            ref code,
            discover: false,
            ..
        } if address.as_deref() == Some("192.168.1.20:17879")
            && name.as_deref() == Some("Studio")
            && code.as_deref() == Some("4821-9034")
    ));
    assert!(matches!(
        machines(&["add", "--discover"]).unwrap(),
        MachineCommands::Add {
            discover: true,
            address: None,
            ..
        }
    ));
    assert!(machines(&["add", "10.0.0.2", "--discover"]).is_err());
    assert!(matches!(
        machines(&["remove", "Studio"]).unwrap(),
        MachineCommands::Remove { ref machine, local: false } if machine == "Studio"
    ));
    assert!(matches!(
        machines(&["remove", "Studio", "--local"]).unwrap(),
        MachineCommands::Remove { local: true, .. }
    ));
    assert!(matches!(
        machines(&["revoke-everywhere", "Laptop"]).unwrap(),
        MachineCommands::RevokeEverywhere { ref machine } if machine == "Laptop"
    ));
    assert!(machines(&["discover", "--json", "--secs", "2"]).is_ok());
    assert!(machines(&["remove"]).is_err());
    assert!(machines(&[]).is_err());
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "machine", "list"])
            .unwrap()
            .command,
        Some(Commands::Machines { .. })
    ));
}

#[test]
fn parses_server_commands() {
    let server = |args: &[&str]| {
        let mut all = vec!["rustcode", "server"];
        all.extend_from_slice(args);
        match Cli::try_parse_from(all).unwrap().command {
            Some(Commands::Server { command }) => command,
            other => panic!("expected a server command, got {other:?}"),
        }
    };
    assert!(matches!(
        server(&["run"]),
        ServerCommands::Run { service: false }
    ));
    assert!(matches!(
        server(&["run", "--service"]),
        ServerCommands::Run { service: true }
    ));
    assert!(matches!(
        server(&["connect"]),
        ServerCommands::Connect {
            or_standalone: false
        }
    ));
    assert!(matches!(
        server(&["connect", "--or-standalone"]),
        ServerCommands::Connect {
            or_standalone: true
        }
    ));
    assert!(matches!(
        server(&["install"]),
        ServerCommands::Install {
            force_dev: false,
            yes: false
        }
    ));
    assert!(matches!(
        server(&["install", "--force-dev"]),
        ServerCommands::Install {
            force_dev: true,
            yes: false
        }
    ));
    assert!(matches!(
        server(&["status", "--json"]),
        ServerCommands::Status { json: true }
    ));
    assert!(matches!(
        server(&["restart", "--when-idle"]),
        ServerCommands::Restart {
            when_idle: true,
            yes: false,
            detached: false
        }
    ));
    assert!(matches!(
        server(&["restart", "-y"]),
        ServerCommands::Restart {
            when_idle: false,
            yes: true,
            detached: false
        }
    ));
    assert!(matches!(
        server(&["restart", "--yes", "--when-idle", "--detached"]),
        ServerCommands::Restart {
            when_idle: true,
            yes: true,
            detached: true
        }
    ));
    assert!(matches!(server(&["link-path"]), ServerCommands::LinkPath));
    assert!(matches!(
        server(&["update-cli"]),
        ServerCommands::UpdateCli { json: false }
    ));
    assert!(matches!(
        server(&["update-cli", "--json"]),
        ServerCommands::UpdateCli { json: true }
    ));
    assert!(matches!(
        server(&["uninstall"]),
        ServerCommands::Uninstall { yes: false }
    ));
    assert!(matches!(
        server(&["logs"]),
        ServerCommands::Logs {
            follow: false,
            lines: 100
        }
    ));
    assert!(matches!(
        server(&["logs", "-f", "--lines", "20"]),
        ServerCommands::Logs {
            follow: true,
            lines: 20
        }
    ));
    assert!(Cli::try_parse_from(["rustcode", "server"]).is_err());
}

#[test]
fn parses_daemon_control_commands() {
    for action in ["start", "run", "stop", "status"] {
        let cli = Cli::try_parse_from(["rustcode", "daemon", action, "--json"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Daemon { .. })));
    }
    let cli = Cli::try_parse_from(["rustcode", "daemon", "logs", "--lines", "25"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Daemon {
            command: DaemonCommands::Logs {
                lines: 25,
                json: false
            }
        })
    ));
}

#[test]
fn parses_cron_add_structured_arguments() {
    let cli = Cli::try_parse_from([
        "rustcode",
        "cron",
        "add",
        "--id",
        "daily-report",
        "--name",
        "Daily report",
        "--workspace",
        "/work",
        "--schedule",
        r#"{"kind":"cron","expression":"0 9 * * *","timezone":"Europe/Oslo"}"#,
        "--action",
        r#"{"type":"prompt","prompt":"Report","workspace":"/work","model_profile":null,"session_id":null}"#,
        "--target-session",
        "session-1",
        "--retry-policy",
        r#"{"max_attempts":2,"initial_backoff_seconds":5,"max_backoff_seconds":30}"#,
        "--json",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::Jobs {
            command: CronCommands::Add {
                id,
                target_session: Some(target),
                json: true,
                ..
            }
        }) if id == "daily-report" && target == "session-1"
    ));
}

#[test]
fn parses_cron_job_controls_and_history_limit() {
    for action in ["pause", "resume", "run", "stop", "delete"] {
        let jobs_cli =
            Cli::try_parse_from(["rustcode", "jobs", action, "job-1", "--json"]).unwrap();
        assert!(matches!(jobs_cli.command, Some(Commands::Jobs { .. })));
        let cli = Cli::try_parse_from(["rustcode", "cron", action, "job-1", "--json"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Jobs { .. })));
        let loops_cli =
            Cli::try_parse_from(["rustcode", "loops", action, "job-1", "--json"]).unwrap();
        assert!(matches!(loops_cli.command, Some(Commands::Jobs { .. })));
    }
    let history = Cli::try_parse_from([
        "rustcode", "cron", "history", "job-1", "--limit", "7", "--json",
    ])
    .unwrap();
    assert!(matches!(
        history.command,
        Some(Commands::Jobs {
            command: CronCommands::History {
                job_id,
                limit: 7,
                json: true
            }
        }) if job_id == "job-1"
    ));
    assert!(Cli::try_parse_from(["rustcode", "jobs", "list", "--json"]).is_ok());
    assert!(Cli::try_parse_from(["rustcode", "cron", "list", "--json"]).is_ok());
    assert!(Cli::try_parse_from(["rustcode", "loops", "list", "--json"]).is_ok());
}

#[test]
fn rejects_incomplete_cron_add_and_unbounded_output_options() {
    assert!(
        Cli::try_parse_from([
            "rustcode",
            "cron",
            "add",
            "--id",
            "job-1",
            "--name",
            "Job",
            "--workspace",
            "/work",
        ])
        .is_err()
    );
    assert!(Cli::try_parse_from(["rustcode", "daemon", "logs", "--lines", "0"]).is_err());
    assert!(
        Cli::try_parse_from(["rustcode", "cron", "history", "job-1", "--limit", "51"]).is_err()
    );
}

#[test]
fn mcp_orchestrator_takes_its_session_socket_and_token() {
    let cli = Cli::try_parse_from([
        "rustcode",
        "mcp-orchestrator",
        "--session",
        "abc",
        "--socket",
        "/tmp/o.sock",
        "--token",
        "t",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::McpOrchestrator { session, socket, token })
            if session == "abc" && socket == std::path::Path::new("/tmp/o.sock") && token.as_deref() == Some("t")
    ));
}

#[test]
fn mcp_connect_takes_its_token_from_the_flag_or_the_environment() {
    let cli = Cli::try_parse_from(["rustcode", "mcp-connect"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::McpConnect { token: None })
    ));
    let cli = Cli::try_parse_from(["rustcode", "mcp-connect", "--token", "rcx_a_b"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Commands::McpConnect { token }) if token.as_deref() == Some("rcx_a_b")
    ));
}

fn mcp_add(args: &[&str]) -> Result<rustcode::config::McpServerConfig, String> {
    let argv = ["rustcode", "mcp", "add"].iter().chain(args).copied();
    match Cli::try_parse_from(argv)
        .map_err(|error| error.to_string())?
        .command
    {
        Some(Commands::Mcp {
            command: McpCommands::Add(args),
        }) => mcp_server_from_args(&args),
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn mcp_add_accepts_a_remote_server_with_trailing_header() {
    let server = mcp_add(&[
        "--transport",
        "http",
        "paral-x",
        "https://mcp.example.test/mcp",
        "--header",
        "Authorization: Bearer token",
    ])
    .unwrap();
    assert_eq!(server.name, "paral-x");
    assert_eq!(server.url.as_deref(), Some("https://mcp.example.test/mcp"));
    assert!(server.command.is_empty());
    assert_eq!(server.headers["Authorization"], "Bearer token");
    assert!(server.enabled);

    assert_eq!(
        mcp_add(&["paral-x", "https://mcp.example.test/mcp"])
            .unwrap()
            .url,
        server.url
    );
}

#[test]
fn mcp_add_accepts_a_stdio_command_with_dashed_arguments() {
    let server = mcp_add(&[
        "--env",
        "API_KEY=k",
        "files",
        "--",
        "npx",
        "-y",
        "files-mcp",
    ])
    .unwrap();
    assert_eq!(server.command, "npx");
    assert_eq!(server.args, ["-y", "files-mcp"]);
    assert_eq!(server.env["API_KEY"], "k");
    assert_eq!(server.url, None);
}

#[test]
fn mcp_add_rejects_options_that_do_not_fit_the_transport() {
    for args in [
        &["--transport", "http", "x", "npx"][..],
        &["x", "https://mcp.example.test", "extra"],
        &["x", "https://mcp.example.test", "--env", "A=b"],
        &["x", "npx", "--header", "A: b"],
        &["x", "https://mcp.example.test", "--header", "no-separator"],
        &["--transport", "sse", "x", "https://mcp.example.test"],
        &["x"],
    ] {
        assert!(mcp_add(args).is_err(), "{args:?}");
    }
}

#[test]
fn parses_capability_commands() {
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "skill", "disable", "review"]).unwrap().command,
        Some(Commands::Skill { command: ToggleCommands::Disable { name } }) if name == "review"
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "mcp", "restart", "files"]).unwrap().command,
        Some(Commands::Mcp { command: McpCommands::Restart { name } }) if name == "files"
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "harness", "default", "codex", "--model", "gpt-5"])
            .unwrap()
            .command,
        Some(Commands::Harness {
            command: HarnessCommands::Default { harness, model: Some(model), effort: None }
        }) if harness == "codex" && model == "gpt-5"
    ));
}

#[test]
fn parses_account_connection_and_outside_commands() {
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "account", "add", "claude-code", "work", "--name", "Work"])
            .unwrap()
            .command,
        Some(Commands::Account {
            command: AccountCommands::Add { harness, id, name: Some(_), config_dir: None }
        }) if harness == "claude-code" && id == "work"
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "connections", "logout", "github", "me", "-y"])
            .unwrap()
            .command,
        Some(Commands::Connections {
            command: ConnectionCommands::Logout { yes: true, .. }
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "outside", "create", "Cursor", "--access", "admin"])
            .unwrap()
            .command,
        Some(Commands::Outside {
            command: OutsideCommands::Create { access: Some(access), .. }
        }) if access == "admin"
    ));
}

#[test]
fn parses_thread_and_worktree_commands() {
    assert!(matches!(
        Cli::try_parse_from([
            "rustcode", "threads", "move", "01ab", "--harness", "codex", "--account", "work"
        ])
        .unwrap()
        .command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Move { id, harness, account: Some(account), model: None })
        }) if id == "01ab" && harness == "codex" && account == "work"
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "threads", "configure", "01ab", "--model", "m", "--effort", "high"])
            .unwrap()
            .command,
        Some(Commands::Sessions {
            command: Some(SessionCommands::Configure { id, model: Some(model), effort: Some(effort), json: false })
        }) if id == "01ab" && model == "m" && effort == "high"
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "threads", "pin", "a", "b"]).unwrap().command,
        Some(Commands::Sessions { command: Some(SessionCommands::Pin { ids }) }) if ids.len() == 2
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "worktree", "remove", "/tmp/w", "--force", "-y"])
            .unwrap()
            .command,
        Some(Commands::Worktree {
            command: WorktreeCommands::Remove {
                force: true,
                yes: true,
                ..
            }
        })
    ));
}

#[test]
fn cli_doc_lists_every_command() {
    use clap::CommandFactory;
    let doc = include_str!("../../../docs/cli.md");
    for command in Cli::command().get_subcommands() {
        let name = command.get_name();
        assert!(
            doc.contains(&format!("`rc {name}")) || doc.contains(&format!("`rustcode {name}")),
            "docs/cli.md has no row for `rc {name}`"
        );
    }
    let root = Cli::command();
    for command in root.find_subcommand("config").unwrap().get_subcommands() {
        let name = command.get_name();
        assert!(
            doc.contains(&format!("`rc config {name}"))
                || doc.contains(&format!("`rc config {name}")),
            "docs/cli.md has no row for `rc config {name}`"
        );
    }
}

#[test]
fn config_commands_are_the_old_top_level_ones() {
    let flat = |args: &[&str]| Cli::try_parse_from(args).unwrap().flatten_config().command;
    assert!(matches!(
        flat(&["rustcode", "config", "sync", "pull"]),
        Some(Commands::Sync {
            command: Some(SyncCommands::Pull),
            ..
        })
    ));
    assert!(matches!(
        flat(&["rustcode", "sync", "--push"]),
        Some(Commands::Sync { push: true, .. })
    ));
    assert!(matches!(
        flat(&["rustcode", "config", "mcp", "list"]),
        Some(Commands::Mcp { .. })
    ));
    assert!(matches!(
        flat(&["rustcode", "mcp", "list"]),
        Some(Commands::Mcp { .. })
    ));
    assert!(matches!(
        flat(&["rustcode", "config", "setting", "list"]),
        Some(Commands::Settings { .. })
    ));
    assert!(matches!(
        flat(&["rustcode", "config", "defaults", "--effort", "high"]),
        Some(Commands::Defaults {
            effort: Some(_),
            ..
        })
    ));
    assert!(Cli::try_parse_from(["rustcode", "config", "sync", "--pull", "--push"]).is_err());
}

#[test]
fn help_groups_cover_every_command() {
    let root = Cli::command();
    let grouped: Vec<&str> = HELP_GROUPS
        .iter()
        .flat_map(|(_, names)| names.iter().copied())
        .collect();
    for command in root.get_subcommands().filter(|c| !c.is_hide_set()) {
        let name = command.get_name();
        assert_eq!(
            grouped.iter().filter(|n| **n == name).count(),
            1,
            "`rustcode {name}` is not in exactly one HELP_GROUPS section"
        );
    }
    for name in grouped {
        assert!(
            root.find_subcommand(name).is_some(),
            "HELP_GROUPS names no command `{name}`"
        );
    }
    let help = command().render_help().to_string();
    for line in [
        "Threads:",
        "Config:",
        "config sync",
        "config mcp",
        "Machines & remote:",
        "Options:",
    ] {
        assert!(
            help.contains(line),
            "`rustcode --help` lacks {line:?}:\n{help}"
        );
    }
    assert!(!help.contains("Commands:"), "{help}");
}

#[test]
fn parses_settings_commands() {
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "settings", "list", "--json"])
            .unwrap()
            .command,
        Some(Commands::Settings {
            command: SettingsCommands::List {
                project: None,
                json: true
            }
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "settings", "set", "continue-after-limit", "false"])
            .unwrap()
            .command,
        Some(Commands::Settings {
            command: SettingsCommands::Set { key, value, project: None, json: false }
        }) if key == "continue-after-limit" && value == "false"
    ));
    assert!(matches!(
        Cli::try_parse_from([
            "rustcode", "setting", "reset", "default-permission-mode", "--project", "/work/a"
        ])
        .unwrap()
        .command,
        Some(Commands::Settings {
            command: SettingsCommands::Reset { key, project: Some(_), .. }
        }) if key == "default-permission-mode"
    ));
}

#[test]
fn parses_usage_and_data_commands() {
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "usage"]).unwrap().command,
        Some(Commands::Usage {
            since: None,
            json: false,
            pull_requests: false,
            prices: None
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "usage", "--pull-requests"])
            .unwrap()
            .command,
        Some(Commands::Usage {
            since: None,
            json: false,
            pull_requests: true,
            prices: None
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "usage", "--pull-requests", "--prices", "p.toml"])
            .unwrap()
            .command,
        Some(Commands::Usage { pull_requests: true, prices: Some(path), .. })
            if path == std::path::Path::new("p.toml")
    ));
    assert!(
        Cli::try_parse_from(["rustcode", "usage", "--prices", "p.toml"]).is_err(),
        "an estimate is only offered per merged pull request"
    );
    assert!(
        Cli::try_parse_from([
            "rustcode",
            "usage",
            "--pull-requests",
            "--since",
            "2026-11-01"
        ])
        .is_err(),
        "the report covers every recorded pull request"
    );
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "usage", "--since", "2026-11-01", "--json"])
            .unwrap()
            .command,
        Some(Commands::Usage { since: Some(day), json: true, .. }) if day == "2026-11-01"
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "data", "schema"])
            .unwrap()
            .command,
        Some(Commands::Data {
            command: DataCommands::Schema
        })
    ));
    assert!(matches!(
        Cli::try_parse_from(["rustcode", "data", "query", "SELECT 1", "--json"])
            .unwrap()
            .command,
        Some(Commands::Data {
            command: DataCommands::Query { sql, json: true }
        }) if sql == "SELECT 1"
    ));
    assert!(
        Cli::try_parse_from(["rustcode", "data", "query"]).is_err(),
        "the statement is required"
    );
}

#[test]
fn mcp_list_summary_hides_header_and_environment_values() {
    let mut server = mcp_add(&[
        "api",
        "https://mcp.example.test/mcp",
        "-H",
        "Authorization: Bearer hunter2",
        "-H",
        "X-Org: acme-secret",
    ])
    .unwrap();
    assert_eq!(
        mcp_server_summary(&server),
        "api  http  https://mcp.example.test/mcp  (headers: Authorization, X-Org)"
    );
    server.enabled = false;
    assert!(mcp_server_summary(&server).ends_with("[disabled]"));

    let stdio = mcp_add(&["-e", "API_KEY=hunter2", "files", "--", "npx", "-y", "m"]).unwrap();
    assert_eq!(
        mcp_server_summary(&stdio),
        "files  stdio  npx -y m  (env: API_KEY)"
    );
}
