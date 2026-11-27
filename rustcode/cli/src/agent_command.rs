//! `rc tools`, `rc secret` and `rc terminal`: what an agent runs from its
//! thread's shell. The first two call the thread's RustCode tools through
//! `rustcode::cli_bridge`, with the approvals and private forms its MCP
//! tools get; `rc terminal` keeps long-lived commands running between calls.

use rustcode::cli_bridge::Bridge;
use serde_json::{Value, json};

#[derive(clap::Subcommand, Debug)]
pub enum AgentCommands {
    /// Call this thread's RustCode tools from its shell, with the same
    /// approvals as its MCP tools and no context cost until called
    #[command(after_help = "Examples:
  rc tools list
  rc tools show worktree_status
  rc tools call list_threads '{\"status\":\"active\"}'
  echo '{\"session_id\":\"abc\"}' | rc tools call worktree_status -")]
    Tools {
        #[command(subcommand)]
        command: ToolsCommand,
    },
    /// Ask the user for credentials in a private form; the values go to a
    /// dotenv file and never reach the agent
    #[command(after_help = "Example:
  rc secret request STRIPE_KEY STRIPE_WEBHOOK_SECRET --reason \"Run the payment tests\" --file .env.local")]
    Secret {
        #[command(subcommand)]
        command: SecretCommand,
    },
    /// Keep long-lived commands (dev servers, watchers) running in the
    /// background, read their output and wait for it
    #[command(after_help = "Examples:
  rc terminal start dev -- bun run dev
  rc terminal wait dev --contains \"Local:\" --timeout 120
  rc terminal output dev --since 2048
  rc terminal stop dev")]
    Terminal {
        #[command(subcommand)]
        command: crate::terminal_command::TerminalCommand,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum ToolsCommand {
    /// List every tool this thread can call, the ones its MCP list leaves out included
    List {
        /// Print the MCP tool entries as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print one tool's description and the JSON arguments it takes
    Show { name: String },
    /// Call a tool with one JSON object of arguments (`-` reads it from stdin)
    Call {
        name: String,
        args: Option<String>,
        /// Print `{"success", "content"}` instead of the content alone
        #[arg(long)]
        json: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum SecretCommand {
    /// Ask for each name in its own private form and write the answers to a
    /// dotenv file; stops at the first one the user declines
    Request {
        #[arg(required = true)]
        names: Vec<String>,
        /// What the credentials are for, shown in the form
        #[arg(long)]
        reason: String,
        /// The dotenv file to write, relative to the thread's folder; it must
        /// not be tracked by Git
        #[arg(long)]
        file: String,
    },
}

/// Run one agent command; the process exit code.
pub async fn run(command: &AgentCommands) -> i32 {
    match command {
        AgentCommands::Tools { command } => tools(command).await,
        AgentCommands::Secret { command } => secret(command).await,
        AgentCommands::Terminal { command } => crate::terminal_command::run(command).await,
    }
}

fn bridge() -> Result<Bridge, i32> {
    Bridge::from_env().map_err(|error| {
        eprintln!("{error}");
        1
    })
}

async fn tools(command: &ToolsCommand) -> i32 {
    let bridge = match bridge() {
        Ok(bridge) => bridge,
        Err(code) => return code,
    };
    match command {
        ToolsCommand::List { json } => {
            let tools = bridge.tools();
            if *json {
                println!("{}", Value::Array(tools));
            } else {
                for tool in &tools {
                    println!("{}", list_line(tool));
                }
            }
            0
        }
        ToolsCommand::Show { name } => match find(&bridge.tools(), name) {
            Some(tool) => {
                println!("{}", show(tool));
                0
            }
            None => {
                eprintln!("error: no tool named {name}; `rc tools list` lists them");
                1
            }
        },
        ToolsCommand::Call { name, args, json } => {
            let args = match read_args(args.as_deref()) {
                Ok(args) => args,
                Err(error) => {
                    eprintln!("error: {error}");
                    return 1;
                }
            };
            let answer = bridge.call(name, args).await;
            if *json {
                println!(
                    "{}",
                    json!({ "success": answer.success, "content": answer.content })
                );
            } else if answer.success {
                println!("{}", answer.content);
            } else {
                eprintln!("{}", answer.content);
            }
            i32::from(!answer.success)
        }
    }
}

async fn secret(command: &SecretCommand) -> i32 {
    let SecretCommand::Request {
        names,
        reason,
        file,
    } = command;
    let bridge = match bridge() {
        Ok(bridge) => bridge,
        Err(code) => return code,
    };
    for name in names {
        let answer = bridge
            .call(
                "request_secret",
                json!({ "name": name, "reason": reason, "destination": file }),
            )
            .await;
        if !answer.success {
            eprintln!("{}", answer.content);
            return 1;
        }
        println!("{}", answer.content);
        if answer.content.starts_with("Declined") {
            return 2;
        }
    }
    0
}

fn find<'a>(tools: &'a [Value], name: &str) -> Option<&'a Value> {
    tools
        .iter()
        .find(|tool| tool["name"].as_str() == Some(name))
}

/// `name  first sentence of the description`, for `rc tools list`.
fn list_line(tool: &Value) -> String {
    let name = tool["name"].as_str().unwrap_or_default();
    let description = tool["description"].as_str().unwrap_or_default();
    let first = description
        .split_once(". ")
        .map_or(description, |(first, _)| first)
        .trim_end_matches('.');
    format!("{name:<28} {first}")
}

fn show(tool: &Value) -> String {
    let schema = serde_json::to_string_pretty(&tool["inputSchema"]).unwrap_or_default();
    format!(
        "{}\n\n{}\n\nArguments (JSON Schema):\n{schema}",
        tool["name"].as_str().unwrap_or_default(),
        tool["description"].as_str().unwrap_or_default()
    )
}

fn read_args(raw: Option<&str>) -> Result<Value, String> {
    let text = match raw {
        None => return Ok(json!({})),
        Some("-") => {
            let mut text = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
                .map_err(|error| format!("cannot read the arguments from stdin: {error}"))?;
            text
        }
        Some(text) => text.to_owned(),
    };
    let value: Value = serde_json::from_str(text.trim())
        .map_err(|error| format!("the arguments are not JSON: {error}"))?;
    if value.is_object() {
        Ok(value)
    } else {
        Err("the arguments must be one JSON object, like '{\"name\":\"value\"}'".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn arguments_must_be_one_object() {
        assert_eq!(read_args(None).unwrap(), json!({}));
        assert_eq!(read_args(Some(r#"{"a":1}"#)).unwrap(), json!({"a": 1}));
        assert!(read_args(Some("[1]")).is_err());
        assert!(read_args(Some("nope")).is_err());
    }

    #[test]
    fn a_listed_tool_shows_its_first_sentence() {
        let tool = json!({"name": "list_threads", "description": "List threads. More detail."});
        assert!(list_line(&tool).starts_with("list_threads"));
        assert!(list_line(&tool).ends_with("List threads"));
    }

    #[test]
    fn parses_the_agent_commands() {
        use crate::cli::{Cli, Commands};
        let parse =
            |args: &[&str]| Cli::try_parse_from(args.iter().copied()).map(|cli| cli.command);
        assert!(matches!(
            parse(&["rc", "tools", "call", "list_threads", "{}"]),
            Ok(Some(Commands::Agent(AgentCommands::Tools {
                command: ToolsCommand::Call { .. }
            })))
        ));
        assert!(matches!(
            parse(&[
                "rc", "secret", "request", "A", "B", "--reason", "r", "--file", ".env"
            ]),
            Ok(Some(Commands::Agent(AgentCommands::Secret { .. })))
        ));
        assert!(parse(&["rc", "secret", "request", "A", "--file", ".env"]).is_err());
        assert!(matches!(
            parse(&["rc", "terminal", "start", "dev", "--", "bun", "run", "dev"]),
            Ok(Some(Commands::Agent(AgentCommands::Terminal { .. })))
        ));
    }
}
