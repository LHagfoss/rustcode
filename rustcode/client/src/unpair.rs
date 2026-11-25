//! Ending this computer's pairing on a machine it is about to forget, so that
//! Remove also takes back the access the machine granted this computer.

use crate::{Connector, Machine, TokenStore, needs_new_pairing};
use std::time::Duration;

/// How long Remove waits for the machine before forgetting it here anyway.
pub const UNPAIR_TIMEOUT: Duration = Duration::from_secs(10);

/// What became of this computer's access on the machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpaired {
    /// The machine revoked this computer's device record.
    Ended,
    /// The machine no longer knew this computer: nothing was left to end.
    AlreadyEnded,
    /// No route answered as the machine within the time allowed.
    Unreachable,
    /// The machine answered but did not end the pairing: an older RustCode.
    Unsupported,
}

/// Sign in to `machine` and ask it to end this computer's pairing, within
/// `timeout` for the whole attempt.
pub async fn unpair_machine(
    connector: &Connector,
    machine: &Machine,
    tokens: &TokenStore,
    timeout: Duration,
) -> Unpaired {
    let attempt = async {
        match connector.connect_raced(machine, tokens).await {
            Ok(connected) => match connected.session.unpair(timeout).await {
                Ok(true) => Unpaired::Ended,
                _ => Unpaired::Unsupported,
            },
            Err(error) if needs_new_pairing(&error) => Unpaired::AlreadyEnded,
            Err(_) => Unpaired::Unreachable,
        }
    };
    tokio::time::timeout(timeout, attempt)
        .await
        .unwrap_or(Unpaired::Unreachable)
}

/// What to tell the owner after removing `name`; `None` when the machine was
/// not asked (`--local`).
pub fn removal_message(name: &str, unpaired: Option<Unpaired>) -> String {
    match unpaired {
        Some(Unpaired::Ended | Unpaired::AlreadyEnded) => {
            format!("Removed {name} and ended this computer's access there.")
        }
        Some(Unpaired::Unreachable) => format!(
            "Removed {name} here. {name} was not reachable, so it still lists this computer: run `rc remote revoke` there."
        ),
        Some(Unpaired::Unsupported) => format!(
            "Removed {name} here. {name} runs a RustCode that cannot end the pairing from here, so it still lists this computer: run `rc remote revoke` there."
        ),
        None => format!(
            "Removed {name}. The machine still lists this computer until you revoke it there."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_says_whether_the_machine_still_lists_this_computer() {
        assert!(removal_message("Studio", Some(Unpaired::Ended)).contains("ended"));
        assert!(removal_message("Studio", Some(Unpaired::AlreadyEnded)).contains("ended"));
        for outcome in [
            Some(Unpaired::Unreachable),
            Some(Unpaired::Unsupported),
            None,
        ] {
            assert!(
                removal_message("Studio", outcome).contains("still lists this computer"),
                "{outcome:?}"
            );
        }
    }
}
