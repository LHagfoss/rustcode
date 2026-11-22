//! Which sign-in a model profile uses, as the config file names it. The
//! sign-ins themselves live in [`crate::provider_auth`].

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey,
    ChatGpt,
    #[serde(rename = "github_copilot")]
    GitHubCopilot,
    /// Legacy config compatibility only. Credential resolution rejects this
    /// method; Claude Code CLI subscription access is no longer supported.
    ClaudeCli,
}

impl AuthMethod {
    pub fn is_chatgpt(self) -> bool {
        matches!(self, Self::ChatGpt)
    }
}

impl fmt::Debug for AuthMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ApiKey => "ApiKey",
            Self::ChatGpt => "ChatGpt",
            Self::GitHubCopilot => "GitHubCopilot",
            Self::ClaudeCli => "ClaudeCli",
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRef {
    pub provider: String,
    pub account: String,
    pub method: AuthMethod,
}

impl CredentialRef {
    pub fn is_chatgpt(&self) -> bool {
        self.method.is_chatgpt()
    }

    pub fn is_copilot(&self) -> bool {
        self.provider == "github-copilot" && self.method == AuthMethod::GitHubCopilot
    }

    pub(crate) fn is_claude_cli(&self) -> bool {
        self.provider == "claude" && self.method == AuthMethod::ClaudeCli
    }
}

impl fmt::Debug for CredentialRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialRef")
            .field("provider", &self.provider)
            .field("account", &"[redacted]")
            .field("method", &self.method)
            .finish()
    }
}
