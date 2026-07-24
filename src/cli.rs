use clap::Parser;

#[derive(Parser, Debug)]
#[command(
    name = "rustcode",
    version = "0.1.15",
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
}
