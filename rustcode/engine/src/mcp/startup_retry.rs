use std::time::Duration;

/// Delays between boot-time start attempts for a server that could not be
/// reached, about half a minute in all.
pub(super) const STARTUP_RETRIES: [Duration; 4] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
    Duration::from_secs(16),
];

/// Time allowed for one server's start, its retries included.
pub(super) const STARTUP_BUDGET: Duration = Duration::from_secs(90);

/// Whether a start failed because the network was not reachable, rather than
/// because the server refused us or needs a new sign-in.
pub(super) fn is_unreachable(error: &str) -> bool {
    error.contains("error sending request") || error.starts_with("Failed to connect to MCP server")
}

/// Run `launch` until it succeeds, fails for a reason other than an unreachable
/// network, or runs out of `delays`.
pub(super) async fn launch_until_reachable<F, Fut>(
    name: &str,
    delays: &[Duration],
    mut launch: F,
) -> Result<(), String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let mut delays = delays.iter();
    loop {
        match launch().await {
            Err(error) if is_unreachable(&error) => match delays.next() {
                Some(delay) => {
                    crate::dbg_log!("[mcp] '{name}' unreachable at startup, retrying: {error}");
                    tokio::time::sleep(*delay).await;
                }
                None => return Err(error),
            },
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn startup_errors_from_an_offline_boot_count_as_unreachable() {
        assert!(is_unreachable(
            "Failed to connect to MCP server 'opper-ai' at https://api.opper.ai/mcp: error sending request for url (https://api.opper.ai/mcp) (is the server running and reachable?)"
        ));
        assert!(is_unreachable(
            "OAuth token request failed: error sending request for url (https://api.opper.ai/mcp/oauth/token)"
        ));
        assert!(!is_unreachable(
            "OAuth token request failed (HTTP 400 Bad Request): {\"error\":\"invalid_grant\"}"
        ));
        assert!(!is_unreachable("MCP server 'opper-ai' startup timed out"));
    }

    #[tokio::test]
    async fn an_unreachable_server_is_started_again_once_the_network_is_up() {
        let attempts = AtomicUsize::new(0);
        let result = launch_until_reachable("s", &[Duration::ZERO; 4], || async {
            match attempts.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => Err("Failed to connect to MCP server 's': error sending request".into()),
                _ => Ok(()),
            }
        })
        .await;
        assert_eq!(result, Ok(()));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_server_that_answers_with_an_error_is_not_retried() {
        let attempts = AtomicUsize::new(0);
        let result = launch_until_reachable("s", &[Duration::ZERO; 4], || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err("MCP server 's' requires authentication".to_string())
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_stop_when_the_delays_run_out() {
        let attempts = AtomicUsize::new(0);
        let result = launch_until_reachable("s", &[Duration::ZERO; 2], || async {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err("error sending request".to_string())
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
