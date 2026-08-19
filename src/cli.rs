use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "rustcode",
    version = env!("CARGO_PKG_VERSION"),
    about = "AI-powered agentic coding assistant terminal"
)]
pub struct Cli {
    /// Resume the most recent chat session
    #[arg(short = 'r', long = "resume")]
    pub resume: bool,

    /// Alias for --resume
    #[arg(short = 'c', long = "continue")]
    pub continue_session: bool,

    /// Run a quick prompt non-interactively and exit
    #[arg(short = 'p', long = "prompt")]
    pub prompt: Option<String>,

    /// Override the active AI model name
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// Check for and install the latest Homebrew release, if available
    #[arg(long = "update", alias = "upgrade")]
    pub update: bool,

    /// Run as a headless Agent Client Protocol server over stdio
    #[arg(long = "acp")]
    pub acp: bool,

    /// Automatically approve tool confirmations for this run
    #[arg(long = "yolo")]
    pub yolo: bool,

    /// Create a project-local .rustcode/config.toml from global defaults
    #[arg(long = "init")]
    pub init: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Commands {
    /// Create a project-local .rustcode/config.toml from global defaults
    Init,

    /// Run as a headless Agent Client Protocol server over stdio
    Acp,

    /// Sync config, skills, and sessions with remote Git repository
    Sync {
        #[command(subcommand)]
        command: Option<SyncCommands>,
    },
}

#[derive(clap::Subcommand, Debug)]
pub enum SyncCommands {
    /// Pull latest config and skills from remote
    Pull,
    /// Push local config and skills to remote
    Push,
    /// Initialize remote Git repository for config sync
    Init { remote_url: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_update_and_upgrade_flags() {
        let cli = Cli::try_parse_from(["rustcode", "--update"]).unwrap();
        assert!(cli.update);
        let cli_alias = Cli::try_parse_from(["rustcode", "--upgrade"]).unwrap();
        assert!(cli_alias.update);
    }

    #[test]
    fn parses_acp_flag() {
        let cli = Cli::try_parse_from(["rustcode", "--acp"]).unwrap();
        assert!(cli.acp);
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
}
