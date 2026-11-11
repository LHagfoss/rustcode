//! Install commands selected on the engine host, including for remote clients.
#[derive(Clone, Copy)]
pub(crate) enum Platform {
    Windows,
    MacOs,
    Linux,
}

pub(crate) const HOST: Platform = if cfg!(windows) {
    Platform::Windows
} else if cfg!(target_os = "macos") {
    Platform::MacOs
} else {
    Platform::Linux
};

#[derive(Clone, Copy)]
pub(crate) enum Tool {
    Codex,
    Claude,
    Gemini,
    OpenCode,
    Pi,
    GitHub,
    Ripgrep,
}

pub(crate) const fn hint(tool: Tool, platform: Platform) -> &'static str {
    match (tool, platform) {
        (Tool::Codex, Platform::Windows) => "npm install -g @openai/codex",
        (Tool::Codex, _) => "brew install codex",
        (Tool::Claude, _) => "npm install -g @anthropic-ai/claude-code",
        (Tool::Gemini, _) => "npm install -g @google/gemini-cli",
        (Tool::OpenCode, Platform::Windows) => "npm install -g @opencode/cli",
        (Tool::OpenCode, _) => "curl -fsSL https://opencode.ai/install | bash",
        (Tool::Pi, _) => "npm install -g @earendil-works/pi-coding-agent",
        (Tool::GitHub, Platform::Windows) => "winget install --id GitHub.cli",
        (Tool::GitHub, _) => "brew install gh",
        (Tool::Ripgrep, Platform::Windows) => "winget install BurntSushi.ripgrep.MSVC",
        (Tool::Ripgrep, _) => "brew install ripgrep` / `cargo install ripgrep",
    }
}

pub(crate) const fn host_hint(tool: Tool) -> &'static str {
    hint(tool, HOST)
}

pub(crate) fn provider_key_hint(env_name: &str, platform: Platform) -> String {
    match platform {
        Platform::Windows => format!(
            "In PowerShell, run [Environment]::SetEnvironmentVariable('{env_name}', '<key>', 'User'), then restart RustCode so it inherits the updated environment"
        ),
        _ => format!(
            "export {env_name}=<key> in ~/.zshenv (sourced by all zsh modes) or ~/.zprofile, then restart rustcode from a login shell; interactive-only ~/.zshrc exports are invisible to desktop/systemd/IDE launches"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_commands() {
        for (tool, expected) in [
            (Tool::Codex, "npm install -g @openai/codex"),
            (Tool::Claude, "npm install -g @anthropic-ai/claude-code"),
            (Tool::Gemini, "npm install -g @google/gemini-cli"),
            (Tool::OpenCode, "npm install -g @opencode/cli"),
            (Tool::Pi, "npm install -g @earendil-works/pi-coding-agent"),
            (Tool::GitHub, "winget install --id GitHub.cli"),
            (Tool::Ripgrep, "winget install BurntSushi.ripgrep.MSVC"),
        ] {
            assert_eq!(hint(tool, Platform::Windows), expected);
        }
    }

    #[test]
    fn provider_key_setup_matches_platform() {
        let windows = provider_key_hint("OPENAI_API_KEY", Platform::Windows);
        assert!(windows.contains("SetEnvironmentVariable('OPENAI_API_KEY', '<key>', 'User')"));
        assert!(!windows.contains("zsh"));
        for platform in [Platform::MacOs, Platform::Linux] {
            assert!(
                provider_key_hint("OPENAI_API_KEY", platform)
                    .starts_with("export OPENAI_API_KEY=<key>")
            );
        }
    }

    #[test]
    fn unix_commands_unchanged() {
        for platform in [Platform::MacOs, Platform::Linux] {
            assert_eq!(hint(Tool::Codex, platform), "brew install codex");
            assert_eq!(hint(Tool::GitHub, platform), "brew install gh");
            assert_eq!(
                hint(Tool::OpenCode, platform),
                "curl -fsSL https://opencode.ai/install | bash"
            );
            assert_eq!(
                hint(Tool::Ripgrep, platform),
                "brew install ripgrep` / `cargo install ripgrep"
            );
        }
    }
}
