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

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(clap::Subcommand, Debug)]
pub enum Commands {
    /// Sync config, skills, and sessions with remote Git repository
    Sync { 
        #[command(subcommand)]
        command: Option<SyncCommands> 
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
