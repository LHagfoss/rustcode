//! Starting an npm-published adapter (Claude Code's, Pi's) through
//! `npx --prefer-offline`.

/// npm's flag that starts a cached copy without asking the registry.
const PREFER_OFFLINE: &str = "--prefer-offline";

/// The command to start again with when `error` says npm found no matching
/// version, or `None`. With `--prefer-offline` npm trusts its cached package
/// listing: one fetched before a dependency of the pinned adapter was
/// published fails every start with `ETARGET`, and Retry hits the same cache.
/// Without the flag npm asks the registry, which also refreshes the listing.
pub(crate) fn online_retry(command: &[String], error: &str) -> Option<Vec<String>> {
    if !(error.contains("ETARGET") || error.contains("notarget")) {
        return None;
    }
    if !command.iter().any(|arg| arg == PREFER_OFFLINE) {
        return None;
    }
    Some(
        command
            .iter()
            .filter(|arg| *arg != PREFER_OFFLINE)
            .cloned()
            .collect(),
    )
}

pub(crate) fn with_pi_steering(harness: &super::ResolvedHarness) -> std::borrow::Cow<'_, [String]> {
    use base64::Engine as _;
    use std::borrow::Cow;

    match harness.command.as_slice() {
        [program, .., package]
            if harness.id == "pi" && program == "npx" && package == super::PI_ACP =>
        {
            let loader =
                base64::engine::general_purpose::STANDARD.encode(include_str!("pi_steering.mjs"));
            let mut command = harness.command.clone();
            command.insert(
                1,
                format!("--node-options=--import=data:text/javascript;base64,{loader}"),
            );
            Cow::Owned(command)
        }
        command => Cow::Borrowed(command),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(str::to_owned).collect()
    }

    #[test]
    fn a_stale_cache_retries_without_prefer_offline() {
        let error = "Claude Code: the harness process exited\nnpm ERR! code ETARGET\n\
                     npm ERR! notarget No matching version found for @agentclientprotocol/sdk@2.9.1.";
        assert_eq!(
            online_retry(&words("npx --prefer-offline -y adapter@1.0.0"), error),
            Some(words("npx -y adapter@1.0.0"))
        );
    }

    #[test]
    fn other_failures_and_online_commands_do_not_retry() {
        let command = words("npx --prefer-offline -y adapter@1.0.0");
        assert_eq!(online_retry(&command, "Claude Code: not signed in"), None);
        assert_eq!(
            online_retry(&words("npx -y adapter@1.0.0"), "npm ERR! code ETARGET"),
            None
        );
    }
}
