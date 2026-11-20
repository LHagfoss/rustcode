//! CLI entry points for `rc` (the RustCode orchestrator CLI and desktop app
//! sidecar): ACP server, background server management, threads, config,
//! machines, remote gateway, usage/data queries, and MCP bridges.

fn hydrate_shell_provider_keys() {
    // Cheap path first: read the workspace config to learn which env names
    // the user's profiles actually reference.
    let configured: Vec<String> = std::env::current_dir()
        .ok()
        .map(|workspace| rustcode::config::load_config_for_workspace(&workspace).2)
        .map(|config| {
            config
                .models
                .iter()
                .filter_map(|profile| profile.api_key_env_name())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    rustcode::shell_env::hydrate_provider_keys(&configured);
}
async fn run_daemon_or_cron_command(
    cli_args: &crate::cli::Cli,
) -> Result<bool, Box<dyn std::error::Error>> {
    use rustcode::daemon::{
        client::DaemonClient,
        create_job, format_response,
        lifecycle::DaemonLifecycle,
        protocol::{DaemonRequest, DaemonResponse},
    };

    let Some(command) = cli_args.command.as_ref() else {
        return Ok(false);
    };
    if !matches!(
        command,
        crate::cli::Commands::Daemon { .. } | crate::cli::Commands::Loops { .. }
    ) {
        return Ok(false);
    }
    let config_dir = rustcode::config::get_config_dir().ok_or("config directory unavailable")?;
    let lifecycle = DaemonLifecycle::new(&config_dir);

    match command {
        crate::cli::Commands::Daemon { command } => match command {
            crate::cli::DaemonCommands::Run { json: _ } => lifecycle.run().await?,
            crate::cli::DaemonCommands::Start { json } => {
                let mut child = tokio::process::Command::new(std::env::current_exe()?);
                child
                    .args(["daemon", "run"])
                    .env("RUSTCODE_CONFIG_DIR", &config_dir);
                let status = lifecycle.start(child).await?;
                println!(
                    "{}",
                    format_response("status", &DaemonResponse::Status { status }, *json)?
                );
            }
            crate::cli::DaemonCommands::Stop { json } => match lifecycle.stop().await? {
                Some(status) if *json => println!(
                    "{}",
                    serde_json::json!({"status":"stopped","pid":status.pid,"instance_id":status.instance_id})
                ),
                Some(status) => println!("Daemon stopped (pid {}).", status.pid),
                None if *json => println!("{}", serde_json::json!({"status":"stopped"})),
                None => println!("Daemon is not running."),
            },
            crate::cli::DaemonCommands::Status { json } => match lifecycle.status().await? {
                Some(status) => println!(
                    "{}",
                    format_response("status", &DaemonResponse::Status { status }, *json)?
                ),
                None if *json => println!("{}", serde_json::json!({"status":"stopped"})),
                None => println!("Daemon: stopped"),
            },
            crate::cli::DaemonCommands::Logs { lines, json } => {
                let output = lifecycle.read_log_tail(*lines)?;
                if *json {
                    println!("{}", serde_json::json!({"lines":lines,"log":output}));
                } else {
                    print!("{output}");
                }
            }
        },
        crate::cli::Commands::Loops { command } => {
            let (operation, request, json) = match command {
                crate::cli::CronCommands::Add {
                    id,
                    name,
                    workspace,
                    schedule,
                    action,
                    target_session,
                    retry_policy,
                    json,
                } => (
                    "add",
                    DaemonRequest::Create {
                        job: create_job(
                            id,
                            name,
                            workspace,
                            schedule,
                            action,
                            target_session.as_deref(),
                            retry_policy.as_deref(),
                        )?,
                    },
                    *json,
                ),
                crate::cli::CronCommands::List { json } => ("list", DaemonRequest::List, *json),
                crate::cli::CronCommands::Pause { job_id, json } => (
                    "pause",
                    DaemonRequest::SetPaused {
                        job_id: job_id.clone(),
                        paused: true,
                    },
                    *json,
                ),
                crate::cli::CronCommands::Resume { job_id, json } => (
                    "resume",
                    DaemonRequest::SetPaused {
                        job_id: job_id.clone(),
                        paused: false,
                    },
                    *json,
                ),
                crate::cli::CronCommands::Run { job_id, json } => (
                    "run",
                    DaemonRequest::RunNow {
                        job_id: job_id.clone(),
                        input: None,
                    },
                    *json,
                ),
                crate::cli::CronCommands::History {
                    job_id,
                    limit,
                    json,
                } => (
                    "history",
                    DaemonRequest::History {
                        job_id: job_id.clone(),
                        limit: *limit,
                    },
                    *json,
                ),
                crate::cli::CronCommands::Delete { job_id, json } => (
                    "delete",
                    DaemonRequest::Delete {
                        job_id: job_id.clone(),
                    },
                    *json,
                ),
            };
            let response = DaemonClient::new(lifecycle.socket_path())
                .request(request)
                .await
                .map_err(|error| format!("daemon unavailable: {error}"))?;
            println!("{}", format_response(operation, &response, json)?);
        }
        _ => unreachable!(),
    }
    Ok(true)
}

async fn run_remote_command(
    command: &crate::cli::RemoteCommands,
) -> Result<(), Box<dyn std::error::Error>> {
    use crate::cli::RemoteCommands;
    use rustcode::remote_gateway::{command as remote, lifecycle::RemoteLifecycle};

    let config_dir = rustcode::config::get_config_dir().ok_or("config directory unavailable")?;
    let lifecycle = RemoteLifecycle::new(&config_dir);
    match command {
        RemoteCommands::Start => {
            let (_, _, config) = rustcode::config::load_config();
            let launcher = rustcode::remote_gateway::owner_client::Launcher {
                program: rustcode::remote_gateway::service::executable_path()?,
                config_directory: config_dir.clone(),
                bind: config.remote.bind,
                port: config.remote.port,
                advertise: config.remote.advertise,
                prefer_interface: config.remote.prefer_interface,
                owner: rustcode::remote_gateway::lifecycle::GatewayOwner::App,
            };
            let _ = std::fs::remove_file(config_dir.join("remote/disabled"));
            let status = rustcode::remote_gateway::service::start(&launcher, &lifecycle).await?;
            println!("{}", remote::format_status(Some(&status)));
        }
        RemoteCommands::SessionOwner { session_id } => {
            // Detached mobile sessions for the built-in harness live in the
            // standalone `rustcode` harness binary so `rc` carries no TUI or
            // interactive runtime dependencies.
            let harness_bin = resolve_harness_binary();
            let status = tokio::process::Command::new(&harness_bin)
                .args(["remote", "session-owner", "--session-id", session_id])
                .status()
                .await
                .map_err(|error| {
                    format!(
                        "detached remote session-owner requires the standalone `rustcode` harness binary (tried {}): {error}. Install it with `install.sh --with-harness` (Windows: `install.ps1 -WithHarness`) or `cargo install --path harness`, set RUSTCODE_HARNESS_BIN, or keep the RustCode desktop app running.",
                        harness_bin.display()
                    )
                })?;
            if !status.success() {
                return Err(format!("session owner exited with {status}").into());
            }
        }
        RemoteCommands::Serve {
            bind,
            port,
            advertise,
            prefer_interface,
        } => {
            // A machine that joined a hub keeps its link from here: the one
            // gateway process a machine runs (#3957).
            let stop = tokio_util::sync::CancellationToken::new();
            tokio::spawn(crate::hub_member::run(config_dir.clone(), stop.clone()));
            let _stop_link = stop.drop_guard();
            remote::serve(
                &config_dir,
                bind,
                *port,
                advertise.as_deref(),
                prefer_interface.as_deref(),
            )
            .await?
        }
        RemoteCommands::Pair => {
            let (_, _, config) = rustcode::config::load_config();
            let launcher = rustcode::remote_gateway::owner_client::Launcher {
                program: rustcode::remote_gateway::service::executable_path()?,
                config_directory: config_dir.clone(),
                bind: config.remote.bind,
                port: config.remote.port,
                advertise: config.remote.advertise,
                prefer_interface: config.remote.prefer_interface,
                owner: rustcode::remote_gateway::lifecycle::GatewayOwner::App,
            };
            let _ = std::fs::remove_file(config_dir.join("remote/disabled"));
            rustcode::remote_gateway::service::start(&launcher, &lifecycle).await?;
            use std::io::IsTerminal;
            let style = if std::io::stdout().is_terminal() {
                remote::QrStyle::Ansi
            } else {
                remote::QrStyle::Text
            };
            let before = lifecycle.status().await.ok().flatten();
            let offer = lifecycle.pair().await?;
            println!("{}", remote::format_offer(&offer, style));
            if style == remote::QrStyle::Ansi {
                remote::watch_pairing_in_terminal(&lifecycle, &offer, before.as_ref()).await;
            }
        }
        RemoteCommands::Devices { json, command } => {
            if let Some(command) = command {
                use crate::cli::RemoteDeviceCommands;
                let (device, allow) = match command {
                    RemoteDeviceCommands::Allow { device } => (device, true),
                    RemoteDeviceCommands::Deny { device } => (device, false),
                };
                lifecycle.set_acp(device, allow).await?;
                println!(
                    "ACP access {} for {device}.",
                    if allow { "allowed" } else { "denied" }
                );
                return Ok(());
            }
            let devices = lifecycle.devices()?;
            if *json {
                println!("{}", remote::devices_json(&devices));
            } else {
                println!("{}", remote::format_devices(&devices));
            }
        }
        RemoteCommands::Accept { id } => {
            let answer = lifecycle.answer_connect_request(id, true).await?;
            println!("{}", remote::format_connect_answer(&answer));
        }
        RemoteCommands::Deny { id } => {
            let answer = lifecycle.answer_connect_request(id, false).await?;
            println!("{}", remote::format_connect_answer(&answer));
        }
        RemoteCommands::Revoke { device } => {
            println!(
                "{}",
                remote::format_revocation(&lifecycle.revoke(device).await?)
            );
        }
        RemoteCommands::Status => {
            println!(
                "{}",
                remote::format_status(lifecycle.status().await?.as_ref())
            );
        }
        RemoteCommands::Stop => {
            if rustcode::remote_gateway::service::disable(&config_dir).await? {
                println!("Remote gateway stopped.");
            } else {
                println!("Remote gateway is not running.");
            }
        }
    }
    Ok(())
}

/// Run `rustcode mcp …`. Edits go to the user config; the workspace's merged
/// view is consulted only to say when a project file hides the change.
fn run_mcp_command(
    command: &crate::cli::McpCommands,
    workspace: &std::path::Path,
) -> Result<(), String> {
    let active = |name: &str| {
        let (_, _, config) = rustcode::config::load_config_for_workspace(workspace);
        config.mcp_servers.iter().any(|server| server.name == name)
    };
    match command {
        crate::cli::McpCommands::Add(args) => {
            let server = crate::cli::mcp_server_from_args(args)?;
            let summary = crate::cli::mcp_server_summary(&server);
            let replaced = rustcode::config::add_mcp_server(server, args.force)?;
            println!(
                "{} MCP server: {summary}",
                if replaced { "Replaced" } else { "Added" }
            );
            if !active(&args.name) {
                println!(
                    "Note: a project .rustcode/config.toml sets its own mcp_servers, so '{}' is not active in this workspace.",
                    args.name
                );
            }
        }
        crate::cli::McpCommands::List => {
            let (_, _, config) = rustcode::config::load_config_for_workspace(workspace);
            if config.mcp_servers.is_empty() {
                println!("No MCP servers configured. Add one with `rustcode mcp add`.");
            }
            for server in &config.mcp_servers {
                println!("{}", crate::cli::mcp_server_summary(server));
            }
        }
        crate::cli::McpCommands::Enable { .. }
        | crate::cli::McpCommands::Disable { .. }
        | crate::cli::McpCommands::Restart { .. } => {
            unreachable!("toggles and restarts are handled by manage_command")
        }
        crate::cli::McpCommands::Remove { name } => {
            rustcode::config::remove_mcp_server(name)?;
            println!("Removed MCP server: {name}");
            if active(name) {
                println!(
                    "Note: a project .rustcode/config.toml still defines '{name}' for this workspace."
                );
            }
        }
    }
    Ok(())
}

/// `rustcode theme ...`: print the result, or the error and exit 1.
fn run_theme_command(command: &crate::cli::ThemeCommands) {
    use crate::cli::ThemeCommands;
    use rustcode::controller::themes::cli;
    let workspace = std::env::current_dir().ok();
    let workspace = workspace.as_deref();
    let result = match command {
        ThemeCommands::List => Ok(cli::list(workspace)),
        ThemeCommands::Show { name, json } => cli::show(name, *json, workspace),
        ThemeCommands::New { name, from } => cli::new(name, from.as_deref(), workspace),
        ThemeCommands::Set { name } => cli::set(name, workspace),
        ThemeCommands::Path { name } => cli::path(name.as_deref(), workspace),
        ThemeCommands::Validate { target } => {
            let (report, ok) = cli::validate_command(target.as_deref(), workspace);
            print!("{report}");
            if !ok {
                std::process::exit(1);
            }
            Ok(String::new())
        }
    };
    match result {
        Ok(text) => print!("{text}"),
        Err(message) => {
            eprintln!("rustcode theme: {message}");
            std::process::exit(1);
        }
    }
}

fn run_storage_command(
    command: &crate::cli::StorageCommands,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        crate::cli::StorageCommands::Status { json } => {
            let Some(config_dir) = rustcode::config::get_config_dir() else {
                eprintln!("Configuration directory is unavailable.");
                std::process::exit(1);
            };
            let status = rustcode::storage_migrations::status(&config_dir);
            if *json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print!("{}", status.render());
            }
        }
        crate::cli::StorageCommands::Usage { json } => {
            let Some(config_dir) = rustcode::config::get_config_dir() else {
                eprintln!("Configuration directory is unavailable.");
                std::process::exit(1);
            };
            let usage = rustcode::storage_report::usage(&config_dir);
            if *json {
                println!("{}", serde_json::to_string_pretty(&usage)?);
            } else {
                print!("{}", usage.render());
            }
        }
        crate::cli::StorageCommands::Clean { apply, json } => {
            let Some(config_dir) = rustcode::config::get_config_dir() else {
                eprintln!("Configuration directory is unavailable.");
                std::process::exit(1);
            };
            let plan = rustcode::storage_cleanup::plan(
                &config_dir,
                rustcode::storage_cleanup::Policy::default(),
            );
            let result = apply.then(|| rustcode::storage_cleanup::apply(&plan));
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "plan": plan,
                        "result": result,
                    }))?
                );
            } else if let Some(result) = &result {
                print!("{}", result.render());
            } else {
                print!("{}", plan.render());
            }
            if result.is_some_and(|result| !result.errors.is_empty()) {
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

/// `rustcode usage --pull-requests`: the `pr_outcomes_by_model` view (#3907).
/// With `prices`, an opt-in estimate (#3934) is added next to, never into,
/// the reported cost.
fn run_pull_request_usage_command(
    json: bool,
    prices: Option<&std::path::Path>,
) -> Result<(), String> {
    let prices = prices
        .map(crate::price_estimate::PriceTable::load)
        .transpose()?;
    rustcode::data_store::ensure_ready()?;
    let mut answer = rustcode::data_store::pull_request_outcomes()?;
    let priced = prices
        .as_ref()
        .map(|table| table.annotate(&mut answer.rows));
    if json {
        let mut body = serde_json::json!({ "rows": answer.rows });
        if let (Some(table), Some(priced)) = (&prices, priced) {
            body["estimate"] = serde_json::json!({
                "source": table.source,
                "as_of": table.as_of,
                "models_priced": priced,
                "note": "RustCode's estimate from this price file, not a cost the harness reported",
            });
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&body).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if answer.rows.is_empty() {
        println!(
            "No linked pull requests recorded yet. A thread's pull request is recorded once it is linked (manage_pull_links or merge_when_green) and the watch reads it."
        );
        return Ok(());
    }
    let dash = || "—".to_owned();
    let text = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
            .map_or_else(dash, str::to_owned)
    };
    let count = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(serde_json::Value::as_i64)
            .map_or_else(dash, thousands)
    };
    let real =
        |row: &serde_json::Value, key: &str| row.get(key).and_then(serde_json::Value::as_f64);
    let table: Vec<Vec<String>> = answer
        .rows
        .iter()
        .map(|row| {
            let mut cells = vec![
                text(row, "harness"),
                text(row, "model"),
                count(row, "pull_requests"),
                count(row, "merged"),
                count(row, "closed_unmerged"),
                real(row, "merge_share")
                    .map_or_else(dash, |share| format!("{:.0}%", share * 100.0)),
                real(row, "avg_ci_failures").map_or_else(dash, |value| format!("{value:.1}")),
                real(row, "avg_fix_rounds").map_or_else(dash, |value| format!("{value:.1}")),
                count(row, "reverted"),
                real(row, "cost_per_merged_pr").map_or_else(dash, |cost| format!("${cost:.2}")),
                count(row, "tokens_per_merged_pr"),
            ];
            if prices.is_some() {
                cells.push(
                    real(row, "estimated_cost_per_merged_pr")
                        .map_or_else(dash, |cost| format!("~${cost:.2}")),
                );
            }
            cells
        })
        .collect();
    println!("RustCode usage · linked pull requests per harness and model");
    println!("Cost is reported by the harness, blank when it reports none.");
    match &prices {
        Some(table) => println!(
            "Estimated cost (~$) is RustCode's estimate from {}, prices as of {}; it is not what the harness reported. A model the file does not price shows —.\n",
            table.source.trim(),
            table.as_of.trim()
        ),
        None => println!(
            "RustCode does not price requests; pass --prices <file> for a labelled estimate.\n"
        ),
    }
    let mut headers = vec![
        "harness",
        "model",
        "PRs",
        "merged",
        "closed",
        "merge share",
        "CI fails",
        "fix rounds",
        "reverted",
        "cost/merged",
        "tokens/merged",
    ];
    if prices.is_some() {
        headers.push("est. cost/merged");
    }
    print_rows(&headers, &table);
    Ok(())
}

/// `rustcode usage`: tokens per harness and model from the local data store.
/// Disk usage is `storage usage`; this never prices requests.
fn run_usage_command(since: Option<&str>, json: bool) -> Result<(), String> {
    let since = match since {
        Some(value) => Some(rustcode::data_store::parse_since(value)?),
        None => None,
    };
    rustcode::data_store::ensure_ready()?;
    // One aggregation, shared with the engine's `thread_usage` tool.
    let answer =
        rustcode::data_store::usage(since.as_deref(), rustcode::data_store::GroupBy::Model, None)?;
    let rows = answer
        .get("rows")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&answer).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "No turns recorded yet. Run a turn, or open the app once so existing threads are collected."
        );
        return Ok(());
    }
    let count = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    let maybe_count =
        |row: &serde_json::Value, key: &str| row.get(key).and_then(serde_json::Value::as_i64);
    let maybe_cost =
        |row: &serde_json::Value| row.get("cost_usd").and_then(serde_json::Value::as_f64);
    let money = |cost: Option<f64>| match cost {
        Some(cost) if cost > 0.0 && cost < 0.01 => "$<0.01".to_owned(),
        Some(cost) => format!("${cost:.2}"),
        None => "—".to_owned(),
    };
    let tokens = |row: &serde_json::Value, key: &str| match maybe_count(row, key) {
        Some(count) => thousands(count),
        None => "—".to_owned(),
    };
    let table_body: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let model = row
                .get("model")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let has_tokens = [
                "input_tokens",
                "output_tokens",
                "cached_tokens",
                "cache_write_tokens",
            ]
            .iter()
            .any(|key| maybe_count(row, key).is_some());
            let dash = "—".to_owned();
            vec![
                row.get("harness")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                if model.is_empty() {
                    dash.clone()
                } else {
                    model.to_owned()
                },
                if has_tokens {
                    tokens(row, "input_tokens")
                } else {
                    dash.clone()
                },
                if has_tokens {
                    tokens(row, "output_tokens")
                } else {
                    dash.clone()
                },
                if has_tokens {
                    tokens(row, "cached_tokens")
                } else {
                    dash.clone()
                },
                if has_tokens {
                    tokens(row, "cache_write_tokens")
                } else {
                    dash.clone()
                },
                money(maybe_cost(row)),
                thousands(count(row, "turns")),
                thousands(count(row, "turns_without_tokens")),
            ]
        })
        .collect();
    let turns: i64 = rows.iter().map(|row| count(row, "turns")).sum();
    let missing: i64 = rows
        .iter()
        .map(|row| count(row, "turns_without_tokens"))
        .sum();
    // A column no row reported stays blank in the total, not 0.
    let total = |key: &str| {
        let values: Vec<i64> = rows
            .iter()
            .filter_map(|row| maybe_count(row, key))
            .collect();
        (!values.is_empty()).then(|| values.into_iter().sum::<i64>())
    };
    let input = total("input_tokens");
    let output = total("output_tokens");
    let cached = total("cached_tokens");
    let cache_write = total("cache_write_tokens");
    let cost: f64 = rows.iter().filter_map(maybe_cost).sum();
    let any_cost = rows.iter().any(|row| maybe_cost(row).is_some());
    let missing_column = |key: &str| -> i64 { rows.iter().map(|row| count(row, key)).sum() };
    let missing_input = missing_column("turns_without_input_tokens");
    let missing_output = missing_column("turns_without_output_tokens");
    let missing_cached = missing_column("turns_without_cached_tokens");
    let missing_cache_write = missing_column("turns_without_cache_write_tokens");
    let missing_cost = missing_column("turns_without_cost");
    // A total is partial when some turns did not report the column: mark it,
    // as the page does, and explain the marks once under the table.
    let total_cell = |value: Option<i64>, missing: i64| match value {
        Some(value) if missing > 0 && turns > 0 => format!("{}*", thousands(value)),
        Some(value) => thousands(value),
        None => "—".to_owned(),
    };
    match &since {
        Some(day) => println!("RustCode usage · tokens per harness and model · since {day}"),
        None => println!("RustCode usage · tokens per harness and model"),
    }
    println!(
        "{turns} turn{}{}",
        if turns == 1 { "" } else { "s" },
        if missing > 0 {
            format!(" ({missing} with no reported token counts)")
        } else {
            String::new()
        }
    );
    println!(
        "Cost is reported by the harness, blank when it reports none; RustCode does not price requests.\n"
    );
    let mut table_rows = table_body;
    table_rows.push(vec![
        "total".to_owned(),
        String::new(),
        total_cell(input, missing_input),
        total_cell(output, missing_output),
        total_cell(cached, missing_cached),
        total_cell(cache_write, missing_cache_write),
        if any_cost {
            let total = money(Some(cost));
            if missing_cost > 0 && turns > 0 {
                format!("{total}†")
            } else {
                total
            }
        } else {
            "—".to_owned()
        },
        thousands(turns),
        thousands(missing),
    ]);
    print_rows(
        &[
            "harness",
            "model",
            "input",
            "output",
            "cached",
            "cache write",
            "cost (USD)",
            "turns",
            "no in+out",
        ],
        &table_rows,
    );
    let partial: Vec<String> = [
        (missing_input, "input"),
        (missing_output, "output"),
        (missing_cached, "cached"),
        (missing_cache_write, "cache write"),
    ]
    .into_iter()
    .filter(|(missing, _)| *missing > 0 && turns > 0)
    .map(|(missing, column)| format!("{column} {} of {turns}", turns - missing))
    .collect();
    if !partial.is_empty() {
        print!(
            "* reported on fewer than all turns: {}{}",
            partial.join(", "),
            if missing_cost > 0 && any_cost {
                format!("; † cost {} of {turns}", turns - missing_cost)
            } else {
                String::new()
            }
        );
        println!(".");
    } else if missing_cost > 0 && any_cost {
        println!(
            "† cost reported on {} of {turns} turns.",
            turns - missing_cost
        );
    }
    Ok(())
}

/// `rustcode data ...`: read-only access to the local data store.
fn run_data_command(command: &crate::cli::DataCommands) -> Result<(), String> {
    use crate::cli::DataCommands;
    rustcode::data_store::ensure_ready()?;
    match command {
        DataCommands::Query { sql, json } => {
            let result = rustcode::data_store::query(sql, rustcode::data_store::MAX_ROWS)?;
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
                );
                return Ok(());
            }
            let columns: Vec<String> = result.columns.clone();
            let rows: Vec<Vec<String>> = result
                .rows
                .iter()
                .map(|row| {
                    columns
                        .iter()
                        .map(|column| {
                            row.get(column)
                                .map(cell_text)
                                .unwrap_or_else(|| "NULL".to_owned())
                        })
                        .collect()
                })
                .collect();
            if rows.is_empty() {
                println!("No rows.");
            } else {
                let headers: Vec<&str> = columns.iter().map(String::as_str).collect();
                print_rows(&headers, &rows);
            }
            if result.truncated {
                println!(
                    "\nAt most {} rows are shown; narrow the query with LIMIT or WHERE.",
                    rustcode::data_store::MAX_ROWS
                );
            }
        }
        DataCommands::Stats { json } => {
            let stats = rustcode::data_store::stats()?;
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&stats).map_err(|error| error.to_string())?
                );
                return Ok(());
            }
            let objects = stats["objects"].as_array().cloned().unwrap_or_default();
            let rows: Vec<Vec<String>> = objects
                .iter()
                .map(|object| {
                    let text = |key: &str| {
                        object
                            .get(key)
                            .filter(|value| !value.is_null())
                            .map(cell_text)
                            .unwrap_or_else(|| "—".to_owned())
                    };
                    vec![text("type"), text("name"), text("rows"), text("bytes")]
                })
                .collect();
            print_rows(&["type", "name", "rows", "bytes"], &rows);
            if let Some(total) = stats["total_bytes"].as_u64() {
                println!("\ndata.sqlite: {total} bytes");
            }
            return Ok(());
        }
        DataCommands::Schema => {
            let result = rustcode::data_store::query(
                "SELECT type, name, sql FROM sqlite_master
                 WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
                 ORDER BY rowid",
                200,
            )?;
            for row in &result.rows {
                let kind = row
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("table");
                let name = row
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let sql = row
                    .get("sql")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                println!("-- {kind} {name}\n{sql};\n");
            }
            println!(
                "Metadata only: no message text, tool arguments or tool output. \
                 Writes are refused; reads take one statement, at most {} rows and 2 s.",
                rustcode::data_store::MAX_ROWS
            );
        }
    }
    Ok(())
}

fn cell_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_owned(),
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Thousands separators, so a token total is readable at a glance.
fn thousands(number: i64) -> String {
    let digits = number.unsigned_abs().to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if number < 0 { format!("-{out}") } else { out }
}

/// Left-aligned columns, two spaces apart.
pub(crate) fn print_rows(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    let line = |cells: &mut dyn Iterator<Item = &str>| {
        let mut out = String::new();
        for (index, cell) in cells.enumerate() {
            if index > 0 {
                out.push_str("  ");
            }
            out.push_str(cell);
            if index + 1 < widths.len() {
                let pad = widths[index].saturating_sub(cell.chars().count());
                out.push_str(&" ".repeat(pad));
            }
        }
        out
    };
    println!("{}", line(&mut headers.iter().copied()));
    for row in rows {
        println!("{}", line(&mut row.iter().map(String::as_str)));
    }
}

/// Whether this command checks the server service for a definition an older
/// release wrote. `up` and `server install`/`repair` do it themselves and
/// restart in the foreground, `server uninstall` removes it anyway, and on
/// Windows reading the definition asks Task Scheduler, so only `server`
/// commands pay for it there.
fn heals_service_at_start(command: Option<&crate::cli::Commands>) -> bool {
    use crate::cli::{Commands, ServerCommands};
    match command {
        Some(Commands::Up { .. }) => false,
        Some(Commands::Server {
            command:
                ServerCommands::Install { .. }
                | ServerCommands::Repair { .. }
                | ServerCommands::Uninstall { .. },
        }) => false,
        Some(Commands::Server { .. }) => true,
        _ => !cfg!(windows),
    }
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cli_args = crate::cli::parse();
    // Started by an external harness on the engine's behalf: it speaks MCP
    // on stdio and must not run migrations or print anything else.
    if let Some(crate::cli::Commands::McpOrchestrator {
        session,
        socket,
        token,
    }) = cli_args.command.as_ref()
    {
        let token = token
            .clone()
            .or_else(|| std::env::var(rustcode::ORCHESTRATOR_TOKEN_ENV).ok())
            .unwrap_or_default();
        if let Err(error) =
            rustcode::run_mcp_orchestrator(session.clone(), socket.clone(), token).await
        {
            eprintln!("rustcode mcp-orchestrator: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // Started by another app (Claude Desktop, Cursor, Codex) from the snippet
    // Settings › Outside agents gives: MCP on stdio, nothing else on stdout.
    if let Some(crate::cli::Commands::McpConnect { token }) = cli_args.command.as_ref() {
        let token = token
            .clone()
            .or_else(|| std::env::var(rustcode::MCP_CONNECT_TOKEN_ENV).ok())
            .unwrap_or_default();
        if let Err(error) = rustcode::run_mcp_connect(token).await {
            eprintln!("rustcode mcp-connect: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // A `PreToolUse` hook of Claude Code: the hook call on stdin, the refusal
    // (if any) on stdout. It never fails the call itself.
    if matches!(
        cli_args.command.as_ref(),
        Some(crate::cli::Commands::GuardCommand)
    ) {
        let mut input = String::new();
        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut input);
        if let Some(response) = rustcode::command_guard::hook_response(&input) {
            println!("{response}");
        }
        return Ok(());
    }
    if let Some(crate::cli::Commands::Theme { command }) = cli_args.command.as_ref() {
        run_theme_command(command);
        return Ok(());
    }
    if let Some(crate::cli::Commands::Storage { command }) = cli_args.command.as_ref() {
        run_storage_command(command)?;
        return Ok(());
    }

    // Apply user-data migrations a newer release introduced, once, before
    // anything reads the config directory (#2599). A failure is reported and
    // retried next start; it never stops startup.
    rustcode::storage_migrations::run_at_startup();
    // A server service an older release installed runs `rustcode`, which is
    // the standalone harness since 1.45.0, and fails to start with
    // "unrecognized subcommand 'server'": point it at this `rc`.
    if heals_service_at_start(cli_args.command.as_ref()) {
        rustcode::service::server_service::heal_at_startup();
    }

    if let Some(crate::cli::Commands::Usage {
        since,
        json,
        pull_requests,
        prices,
    }) = cli_args.command.as_ref()
    {
        let result = if *pull_requests {
            run_pull_request_usage_command(*json, prices.as_deref())
        } else {
            run_usage_command(since.as_deref(), *json)
        };
        if let Err(error) = result {
            eprintln!("rustcode usage: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(crate::cli::Commands::Data { command }) = cli_args.command.as_ref() {
        if let Err(error) = run_data_command(command) {
            eprintln!("rustcode data: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some((op, json)) = cli_args
        .command
        .as_ref()
        .and_then(crate::manage_command::capability_op)
    {
        if let Err(error) = crate::manage_command::capability(op, json).await {
            eprintln!("rustcode: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(command) = cli_args.command.as_ref()
        && let Some(result) = crate::manage_command::credentials(command).await
    {
        if let Err(error) = result {
            eprintln!("rustcode: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(crate::cli::Commands::Worktree { command }) = cli_args.command.as_ref() {
        if let Err(error) = crate::manage_command::worktree(command) {
            eprintln!("rustcode worktree: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(crate::cli::Commands::Settings { command }) = cli_args.command.as_ref() {
        if let Err(error) = crate::manage_command::settings(command).await {
            eprintln!("rustcode settings: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if run_daemon_or_cron_command(&cli_args).await? {
        return Ok(());
    }
    if let Some(crate::cli::Commands::Machines { command }) = cli_args.command.as_ref() {
        if let Err(error) = crate::machines::run(command).await {
            eprintln!("rustcode machines: {error:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(crate::cli::Commands::Up {
        yes,
        json,
        no_service,
    }) = cli_args.command.as_ref()
    {
        crate::up::run(*yes, *json, *no_service).await;
    }
    if let Some(crate::cli::Commands::Remote { command }) = cli_args.command.as_ref() {
        run_remote_command(command).await?;
        return Ok(());
    }
    if let Some(crate::cli::Commands::Server { command }) = cli_args.command.as_ref()
        && !matches!(command, crate::cli::ServerCommands::Run { .. })
        && !matches!(command, crate::cli::ServerCommands::Connect)
    {
        crate::server_command::run(command).await;
    }
    if let Some(crate::cli::Commands::Server {
        command: crate::cli::ServerCommands::Connect,
    }) = cli_args.command.as_ref()
    {
        // A relay only: the server it reaches, or starts, does the rest.
        let result = rustcode::server::connect(std::env::current_exe()?).await;
        // Standard input is read on a blocking thread that would hold the
        // runtime open: leave now that the relay is over.
        let code = match result {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("rustcode server connect: {error:#}");
                1
            }
        };
        std::process::exit(code);
    }
    if let Some(crate::cli::Commands::Serve {
        port,
        bind,
        token,
        allow_remote,
    }) = cli_args.command.as_ref()
    {
        let workspace = std::env::current_dir()?;
        rustcode::serve::run(bind, *port, token.clone(), *allow_remote, workspace).await?;
        return Ok(());
    }
    // Rotate a debug log left by a previous process before startup emits new
    // diagnostics. The logger also checks the cap before each append so long-
    // lived processes remain bounded.
    rustcode::logger::rotate_if_oversized();
    // Issue #1226: silent mid-stream hangs left zero evidence. A panic hook
    // preserves the message + location in debug.log even when the process
    // dies without a crash report.
    rustcode::logger::install_panic_hook();
    // Provider keys are often exported in `~/.zshrc` (interactive-only) and
    // invisible to GUI/systemd/IDE launches. Hydrate missing keys from the
    // login shell once at startup so profiles, MCP servers, and tool shells
    // all see the same values the user's terminal sees.
    hydrate_shell_provider_keys();

    if cli_args.init || matches!(cli_args.command.as_ref(), Some(crate::cli::Commands::Init)) {
        let workspace = std::env::current_dir()?;
        match rustcode::config::init_project_config(&workspace) {
            Ok(path) => println!(
                "Created project config at {}\nAdded {} to .gitignore.",
                path.display(),
                ".rustcode/config.toml"
            ),
            Err(error) => {
                eprintln!("Project config initialization failed: {error}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Export { id, out }) = cli_args.command.as_ref() {
        run_session_export(id.as_deref(), out.as_deref());
        return Ok(());
    }

    if let Some(crate::cli::Commands::Sessions { command }) = cli_args.command.as_ref() {
        match command {
            Some(crate::cli::SessionCommands::Export { id, out }) => {
                run_session_export(id.as_deref(), out.as_deref());
            }
            Some(crate::cli::SessionCommands::Migrate { dry_run }) => {
                let Some(report) = rustcode::config::migrate_legacy_sessions(*dry_run) else {
                    eprintln!("Thread migration failed: configuration directory is unavailable.");
                    std::process::exit(1);
                };
                println!(
                    "{} {} legacy thread(s): {} migrated, {} skipped, {} error(s).",
                    if *dry_run {
                        "Would migrate"
                    } else {
                        "Processed"
                    },
                    report.found,
                    report.migrated,
                    report.skipped,
                    report.errors.len()
                );
                for error in &report.errors {
                    eprintln!("thread migration: {error}");
                }
                if !report.errors.is_empty() {
                    std::process::exit(1);
                }
            }
            Some(crate::cli::SessionCommands::Settle { ids }) => {
                run_sessions_set_status(ids, ThreadStatusChange::Settle);
            }
            Some(crate::cli::SessionCommands::Unsettle { ids }) => {
                run_sessions_set_status(ids, ThreadStatusChange::Unsettle);
            }
            Some(crate::cli::SessionCommands::Review { ids }) => {
                run_sessions_set_status(ids, ThreadStatusChange::Review);
            }
            Some(
                command @ (crate::cli::SessionCommands::Move { .. }
                | crate::cli::SessionCommands::Rename { .. }
                | crate::cli::SessionCommands::Pin { .. }
                | crate::cli::SessionCommands::Unpin { .. }),
            ) => {
                if let Err(error) = crate::manage_command::thread(command).await {
                    eprintln!("rustcode threads: {error}");
                    std::process::exit(1);
                }
            }
            // `rustcode sessions` with no subcommand lists sessions, scoped
            // to the current workspace by default (`list --all` shows every
            // workspace with its path per entry).
            Some(crate::cli::SessionCommands::List { all }) => {
                run_sessions_list(*all || cli_args.all);
            }
            Some(crate::cli::SessionCommands::Search {
                query,
                all,
                limit,
                json,
            }) => {
                run_sessions_search(query, *all || cli_args.all, *limit, *json);
            }
            Some(crate::cli::SessionCommands::Path { id }) => {
                match rustcode::config::get_active_session_dir(id) {
                    Some(dir) if dir.is_dir() => println!("{}", dir.display()),
                    Some(_) => {
                        eprintln!("No saved thread with id `{id}`.");
                        std::process::exit(1);
                    }
                    None => {
                        eprintln!("Configuration directory is unavailable.");
                        std::process::exit(1);
                    }
                }
            }
            None => {
                run_sessions_list(cli_args.all);
            }
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Doctor { fix }) = cli_args.command.as_ref() {
        let code = rustcode::doctor::run_doctor(*fix);
        if code != 0 {
            std::process::exit(code);
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Bench {
        report,
        baseline,
        rounds,
        calls,
        recoveries,
        completed,
    }) = cli_args.command.as_ref()
    {
        if let Some(path) = report {
            let measured: rustcode::benchmark::TurnPerformance =
                serde_json::from_slice(&std::fs::read(path)?)?;
            println!("{}", measured.report());
            if let Some(path) = baseline {
                let original = serde_json::from_slice(&std::fs::read(path)?)?;
                println!(
                    "{}",
                    rustcode::benchmark::compare_reports(&original, &measured)
                );
            }
            return Ok(());
        }
        let stats = rustcode::benchmark::TurnStats {
            rounds: *rounds,
            tool_calls: *calls,
            recoveries: *recoveries,
            completed: *completed,
        };
        println!("{}", rustcode::benchmark::format_report(stats));
        return Ok(());
    }

    if let Some(crate::cli::Commands::Mcp { command }) = cli_args.command.as_ref()
        && let Some(op) = crate::manage_command::mcp_op(command)
    {
        if let Err(error) = crate::manage_command::capability(op, false).await {
            eprintln!("rustcode mcp: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(crate::cli::Commands::Mcp { command }) = cli_args.command.as_ref() {
        if let Err(error) = run_mcp_command(command, &std::env::current_dir()?) {
            eprintln!("rustcode mcp: {error}");
            std::process::exit(1);
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Defaults { model, effort }) = cli_args.command.as_ref() {
        let workspace = std::env::current_dir()?;
        let (_, _, mut config) = rustcode::config::load_config_for_workspace(&workspace);
        if let Err(error) =
            rustcode::model_defaults::save(&mut config, model.as_deref(), effort.as_deref())
        {
            eprintln!("rustcode defaults: {error}");
            std::process::exit(1);
        }
        println!(
            "{}",
            serde_json::to_string(&rustcode::model_defaults::report(&config))?
        );
        return Ok(());
    }

    if let Some(crate::cli::Commands::Discord {
        setup,
        status: _status,
        enable,
        disable,
    }) = cli_args.command.as_ref()
    {
        let workspace = std::env::current_dir()?;
        let (_, _, mut config) = rustcode::config::load_config_for_workspace(&workspace);
        if *setup || *enable || *disable {
            config.discord_rpc_enabled = !*disable;
            rustcode::config::save_entire_config(&config);
            println!(
                "Discord Rich Presence {}.",
                if config.discord_rpc_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            if *setup {
                println!(
                    "Keep the Discord desktop app running and ensure the RustCode application has a `rustcode_logo` large asset."
                );
                println!(
                    "Rich Presence uses Discord's local desktop IPC; RustCode has no Discord login flow and never uses passwords, tokens, or browser profiles."
                );
            }
        } else {
            let socket = rustcode::discord_rpc::ipc_socket_detected();
            println!(
                "Discord Rich Presence: {}",
                if config.discord_rpc_enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            println!(
                "Discord desktop IPC: {}",
                if socket {
                    "socket detected"
                } else {
                    "not detected (Discord may be closed)"
                }
            );
            if let Some(path) = rustcode::config::get_config_dir() {
                println!("Config directory: {}", path.display());
            }
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Sync {
        command,
        pull,
        push,
    }) = cli_args.command
    {
        let action = match crate::cli::resolve_sync_action(command, pull, push) {
            Ok(action) => action,
            Err(error) => {
                eprintln!("Sync argument error: {error}");
                std::process::exit(2);
            }
        };

        match action {
            crate::cli::SyncAction::Pull => {
                println!(
                    "📥 [sync] Pulling latest config, skills, and themes from remote origin..."
                );
                if let Err(e) = rustcode::config::sync_config_pull() {
                    eprintln!("Sync pull failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            crate::cli::SyncAction::Push => {
                println!("💾 [sync] Staging and pushing config, skills, and themes...");
                if let Err(e) = rustcode::config::sync_config_push() {
                    eprintln!("Sync push failed: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
            crate::cli::SyncAction::Init(remote_url) => {
                if let Err(e) = rustcode::config::init_sync_repo(&remote_url) {
                    eprintln!("Error initializing sync repo: {e}");
                    std::process::exit(1);
                }
                println!(
                    "Sync repository setup complete! You can now run `rustcode config sync` anytime."
                );
            }
            crate::cli::SyncAction::PullThenPush => {
                // Default behavior for `rustcode config sync` (pull then push)
                println!(
                    "📥 [sync] Pulling latest config, skills, and themes from remote origin..."
                );
                if let Err(e) = rustcode::config::sync_config_pull() {
                    eprintln!("Sync failed during pull: {e}");
                    std::process::exit(1);
                }
                println!("💾 [sync] Staging and pushing config, skills, and themes...");
                if let Err(e) = rustcode::config::sync_config_push() {
                    eprintln!("Sync failed during push: {e}");
                    std::process::exit(1);
                }
                println!("✅ [sync] Config sync complete!");
            }
        }
        return Ok(());
    }

    if cli_args.update {
        println!("Checking if new release...");
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()?;
        let check = match rustcode::update::check_for_update(&client).await {
            Ok(check) => check,
            Err(error) => {
                eprintln!("Update check failed: {error}");
                std::process::exit(1);
            }
        };

        match check {
            rustcode_core::update::UpdateCheck::UpToDate { current, latest } => {
                println!(
                    "No new release. rustcode v{} is up to date (latest: v{}).",
                    rustcode_core::update::format_version(current),
                    rustcode_core::update::format_version(latest)
                );
            }
            rustcode_core::update::UpdateCheck::Available { current, latest } => {
                println!(
                    "Found new release: v{} → v{}, updating now...",
                    rustcode_core::update::format_version(current),
                    rustcode_core::update::format_version(latest)
                );
                match rustcode::update::run_update(&client, latest).await {
                    Ok(()) => {
                        println!("🎉 Update ran successfully! Please restart rustcode.");
                    }
                    Err(error) => {
                        eprintln!("Update failed: {error}");
                        std::process::exit(1);
                    }
                }
            }
        }
        return Ok(());
    }

    if let Some(crate::cli::Commands::Server {
        command: crate::cli::ServerCommands::Run { service },
    }) = cli_args.command.as_ref()
    {
        // Started by launchd, systemd or `server connect` with a bare
        // environment, as the app's engine is.
        rustcode::shell_env::hydrate_all_missing();
        match rustcode::server::run(cli_args.yolo, *service).await? {
            rustcode::server::RunOutcome::Stopped => rustcode::config::flush_history(),
            rustcode::server::RunOutcome::AlreadyRunning(Some(info)) => println!(
                "A RustCode server is already running for this user: pid {}, version {}, at {}.",
                info.pid, info.version, info.endpoint
            ),
            rustcode::server::RunOutcome::AlreadyRunning(None) => {
                println!("A RustCode server is already starting for this user.")
            }
        }
        return Ok(());
    }

    let acp_subcommand = matches!(cli_args.command.as_ref(), Some(crate::cli::Commands::Acp));
    if cli_args.acp || acp_subcommand {
        // The macOS app launches the engine with the bare launchd environment;
        // give MCP servers and tool shells everything the login shell exports.
        rustcode::shell_env::hydrate_all_missing();
        rustcode::acp::run_acp(cli_args.yolo).await?;
        rustcode::config::flush_history();
        return Ok(());
    }

    // `rc` is the orchestrator CLI (the interactive TUI lives in the standalone
    // `rustcode` harness binary). Print grouped help when invoked with no command.
    crate::cli::command().print_help()?;
    println!();
    Ok(())
}

/// Locate the standalone `rustcode` harness binary for commands that delegate
/// to it (currently detached mobile `remote session-owner`): explicit
/// `$RUSTCODE_HARNESS_BIN`, else a sibling beside `rc`, else `rustcode` on `PATH`.
fn resolve_harness_binary() -> std::path::PathBuf {
    if let Some(path) = std::env::var_os("RUSTCODE_HARNESS_BIN").filter(|value| !value.is_empty()) {
        return std::path::PathBuf::from(path);
    }
    let name = if cfg!(windows) {
        "rustcode.exe"
    } else {
        "rustcode"
    };
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join(name);
        if sibling.is_file() {
            return sibling;
        }
    }
    std::path::PathBuf::from(name)
}

/// `rc export [id]`: zip a thread for another machine. Without an id,
/// the most recent thread of the current workspace is exported.
fn run_session_export(id: Option<&str>, out: Option<&std::path::Path>) {
    let id = match id {
        Some(id) => id.to_owned(),
        None => {
            let scope = rustcode::config::SessionScope::for_current_dir();
            let (sessions, _) = rustcode::config::list_sessions_in_scope(1, &scope);
            match sessions
                .first()
                .and_then(|meta| rustcode::config::session_id_from_path(&meta.path))
            {
                Some(id) => id,
                None => {
                    eprintln!("No saved threads in this workspace; pass a thread id.");
                    std::process::exit(1);
                }
            }
        }
    };
    match rustcode::config::export_session(&id, out) {
        Ok(path) => println!("{}", path.display()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}

/// What `rustcode threads settle|unsettle|review` stores, and the word it
/// prints: the app's Settled, Active and Review over the engine's `done`,
/// `idle` and `review` (`docs/orchestrator.md`, *Thread status*).
#[derive(Clone, Copy)]
enum ThreadStatusChange {
    Settle,
    Unsettle,
    Review,
}

impl ThreadStatusChange {
    /// The status the engine stores (`_rustcode/session/status`).
    fn status(self) -> &'static str {
        match self {
            Self::Settle => "done",
            Self::Unsettle => "idle",
            Self::Review => "review",
        }
    }

    /// The word the CLI prints, the one the app shows.
    fn word(self) -> &'static str {
        match self {
            Self::Settle => "settled",
            Self::Unsettle => "active",
            Self::Review => "review",
        }
    }
}

/// `rustcode threads settle|unsettle|review <id>...`: store each thread's
/// status through the engine's store, so the app's next `session/list` and the
/// terminal picker read the same `status.json`. One line per thread, an
/// unknown or ambiguous id on stderr, and a non-zero exit if any failed.
fn run_sessions_set_status(ids: &[String], change: ThreadStatusChange) {
    let saved = rustcode::config::session_ids();
    let mut failed = false;
    for raw in ids {
        let id = match resolve_thread_id(raw, &saved) {
            Ok(id) => id,
            Err(message) => {
                eprintln!("{message}");
                failed = true;
                continue;
            }
        };
        if matches!(change, ThreadStatusChange::Unsettle)
            && rustcode::config::session_status(&id) == "review"
        {
            // Settling is the way out of review; un-settling is not that, so a
            // mark the user has not looked at yet stays.
            println!("kept {id} review");
            continue;
        }
        if let Err(error) = rustcode::config::set_session_status(&id, change.status()) {
            eprintln!("Could not mark `{raw}` {}: {error}", change.word());
            failed = true;
            continue;
        }
        let title =
            rustcode::config::load_session_title(&id).unwrap_or_else(|| "(untitled)".to_owned());
        println!("{} {id} {title}", change.word());
    }
    if failed {
        std::process::exit(1);
    }
}

/// A thread id, or a unique prefix of one, resolved against the saved threads.
pub(crate) fn resolve_thread_id(raw: &str, saved: &[String]) -> Result<String, String> {
    let wanted = raw.trim();
    if wanted.is_empty() {
        return Err("A thread id is required.".to_owned());
    }
    if let Some(id) = saved.iter().find(|id| id.as_str() == wanted) {
        return Ok(id.clone());
    }
    // A thread the store has a folder for but no transcript yet (settled right
    // after it was started, or one whose history was lost) is still that
    // thread: an exact id may name it, as `threads path` does. A prefix may
    // not, since that would be guessing at a directory that no list vouches for.
    if rustcode::config::get_active_session_dir(wanted).is_some_and(|dir| dir.is_dir()) {
        return Ok(wanted.to_owned());
    }
    let matches: Vec<&String> = saved.iter().filter(|id| id.starts_with(wanted)).collect();
    match matches.as_slice() {
        [] => Err(format!("No saved thread has id `{wanted}`.")),
        [id] => Ok((*id).clone()),
        _ => {
            let shown = matches
                .iter()
                .take(5)
                .map(|id| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let more = matches.len().saturating_sub(5);
            Err(format!(
                "`{wanted}` matches {} threads: {shown}{}. Pass more of the id.",
                matches.len(),
                if more > 0 {
                    format!(", and {more} more")
                } else {
                    String::new()
                }
            ))
        }
    }
}

fn run_sessions_list(show_all: bool) {
    const LIMIT: usize = 50;
    // `--all` lists every workspace; the default scope is the store's shared
    // parent/child rule against the current directory.
    let scope = if show_all {
        rustcode::config::SessionScope::All
    } else {
        rustcode::config::SessionScope::for_current_dir()
    };
    let (sessions, truncated) = rustcode::config::list_sessions_in_scope(LIMIT, &scope);
    if sessions.is_empty() {
        println!("No saved threads found.");
        return;
    }
    for meta in &sessions {
        let id = rustcode::config::session_id_from_path(&meta.path).unwrap_or_default();
        let workspace = meta
            .workspace_cwd
            .as_deref()
            .map(|cwd| cwd.display().to_string())
            .unwrap_or_else(|| "no workspace recorded".to_owned());
        if show_all {
            println!(
                "{id}  {}  {} msgs  {workspace}  {}",
                meta.title, meta.message_count, meta.when
            );
        } else {
            println!(
                "{id}  {}  {} msgs  {}",
                meta.title, meta.message_count, meta.when
            );
        }
    }
    if truncated {
        eprintln!("Showing the {LIMIT} most recent threads; use --all to see every workspace.");
    }
}

/// `rustcode sessions search`: sessions whose messages contain `query`, newest
/// first. `--json` prints an array the desktop app reads.
fn run_sessions_search(query: &str, show_all: bool, limit: usize, json: bool) {
    let scope = if show_all {
        rustcode::config::SessionScope::All
    } else {
        rustcode::config::SessionScope::for_current_dir()
    };
    let hits = rustcode::config::search_sessions(query, limit, &scope);
    if json {
        let rows: Vec<serde_json::Value> = hits
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "id": hit.id,
                    "title": hit.meta.title,
                    "snippet": hit.snippet,
                    "matches": hit.match_count,
                    "when": hit.meta.when,
                    "cwd": hit.meta.workspace_cwd.as_ref().map(|cwd| cwd.display().to_string()),
                })
            })
            .collect();
        println!("{}", serde_json::Value::Array(rows));
        return;
    }
    if hits.is_empty() {
        println!("No saved threads match `{query}`.");
        return;
    }
    for hit in &hits {
        println!(
            "{}  {}  {} match(es)  {}",
            hit.id, hit.meta.title, hit.match_count, hit.snippet
        );
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_thread_id;

    fn saved(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn a_thread_id_resolves_by_prefix_or_in_full() {
        let threads = saved(&["018f0000-aaaa", "018fffff-bbbb"]);
        assert_eq!(
            resolve_thread_id("018f0000-aaaa", &threads),
            Ok("018f0000-aaaa".to_owned())
        );
        assert_eq!(
            resolve_thread_id("018f0000", &threads),
            Ok("018f0000-aaaa".to_owned())
        );
        // A prefix is trimmed before it is matched.
        assert_eq!(
            resolve_thread_id(" 018fffff-bb ", &threads),
            Ok("018fffff-bbbb".to_owned())
        );
    }

    #[test]
    fn an_unknown_thread_id_names_itself_and_fails() {
        let error = resolve_thread_id("nope", &saved(&["018f0000-aaaa"])).unwrap_err();
        assert!(error.contains("No saved thread"), "{error}");
        assert!(error.contains("nope"), "{error}");
        assert!(resolve_thread_id("", &saved(&["018f0000-aaaa"])).is_err());
    }

    #[test]
    fn an_ambiguous_prefix_lists_its_candidates() {
        let threads = saved(&["018f-aaaa", "018f-bbbb"]);
        let error = resolve_thread_id("018f", &threads).unwrap_err();
        assert!(error.contains("matches 2 threads"), "{error}");
        assert!(
            error.contains("018f-aaaa") && error.contains("018f-bbbb"),
            "{error}"
        );
        assert!(error.contains("Pass more of the id"), "{error}");
    }
}
