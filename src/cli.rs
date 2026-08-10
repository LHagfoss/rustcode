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
    #[arg(long = "upgrade")]
    pub upgrade: bool,

    /// Run as a headless Agent Client Protocol server over stdio
    #[arg(long = "acp")]
    pub acp: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Commands {
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
    fn parses_upgrade_flag() {
        let cli = Cli::try_parse_from(["rustcode", "--upgrade"]).unwrap();
        assert!(cli.upgrade);
    }

    #[test]
    fn parses_acp_flag() {
        let cli = Cli::try_parse_from(["rustcode", "--acp"]).unwrap();
        assert!(cli.acp);
    }
}
