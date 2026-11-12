//! What a client does while a paired machine is away.
//!
//! A machine that sleeps, restarts or changes network comes back on its own:
//! the supervisor tries again, waiting a little longer after each failure but
//! never more than [`BACKOFF_MAX`], and it ends a wait early when this
//! computer's own network changes — the VPN came up, the laptop joined
//! another Wi-Fi — because that is what makes a route that was dead work
//! (#3436).
//!
//! It also ends a wait when this computer wakes from sleep ([`slept`]), and
//! names why the last attempt failed ([`Cause`]): a machine that answered and
//! refused is not the same as one that is off, and the app and agents say
//! which.
//!
//! Everything here is a pure function of its arguments, so the schedule, the
//! route order and "did the network change" are unit-tested rather than timed.

use crate::machine::{NotThisMachine, PairingLost, Refusal, Unreachable};
use crate::protocol::ErrorCode;
use crate::registry::Route;
use std::net::IpAddr;
use std::time::Duration;

/// The wait after the first failure, and the longest one. The cap is a minute:
/// long enough that a machine that is off is not dialled in a loop, short
/// enough that a machine that comes back is found while the person is still
/// looking at the row.
pub const BACKOFF_START: Duration = Duration::from_secs(1);
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// The longest wait after `attempt` failures: 1, 2, 4, 8, 16, 32, then a
/// minute. `attempt` counts the failures so far, starting at zero.
pub fn backoff_cap(attempt: u32) -> Duration {
    let seconds = BACKOFF_START
        .as_secs()
        .saturating_mul(1u64.checked_shl(attempt.min(16)).unwrap_or(u64::MAX / 2));
    Duration::from_secs(seconds).min(BACKOFF_MAX)
}

/// The wait before the next attempt: at most [`backoff_cap`] and at least half
/// of it, spread by `jitter` in `0.0..=1.0`, so several machines that lost the
/// same network do not all come back in the same second.
pub fn backoff_delay(attempt: u32, jitter: f64) -> Duration {
    let cap = backoff_cap(attempt);
    let half = cap.as_secs_f64() / 2.0;
    Duration::from_secs_f64(half + half * jitter.clamp(0.0, 1.0))
}

/// The routes in the order they should be tried, with `last` — the route this
/// machine answered on last time — moved to the front of its own class.
///
/// A route that worked a minute ago is the best guess after a dropped
/// connection, but the classes still decide most of the race: the LAN routes
/// are tried before the overlay ones ([`crate::routes::ordered`]), so a laptop
/// that comes home does not dial the VPN first. Stable, so the rest keeps its
/// saved order.
pub fn prefer_last_working(routes: &[Route], last: Option<&str>) -> Vec<Route> {
    let mut ordered = routes.to_vec();
    let Some(last) = last else {
        return ordered;
    };
    let Some(index) = ordered.iter().position(|route| route.address == last) else {
        return ordered;
    };
    let rank = ordered[index].kind.rank();
    let working = ordered.remove(index);
    let first_of_class = ordered
        .iter()
        .position(|route| route.kind.rank() == rank)
        .unwrap_or_else(|| {
            ordered
                .iter()
                .position(|route| route.kind.rank() > rank)
                .unwrap_or(ordered.len())
        });
    ordered.insert(first_of_class, working);
    ordered
}

/// Whether this computer's own network is no longer the one it was: an address
/// appeared or went away (a VPN came up or went down, another Wi-Fi handed out
/// a lease), which is worth trying the machine again *now* rather than at the
/// end of the backoff.
///
/// Both lists are [`crate::routes::local_addresses`]-shaped. An empty list on
/// either side says nothing about the network, so it is not a change: a
/// machine that cannot read its interfaces must not retry in a tight loop.
pub fn network_changed(before: &[IpAddr], after: &[IpAddr]) -> bool {
    if before.is_empty() || after.is_empty() {
        return false;
    }
    let mut before = before.to_vec();
    let mut after = after.to_vec();
    before.sort();
    before.dedup();
    after.sort();
    after.dedup();
    before != after
}

/// How long a connected machine may say nothing at all, pings included,
/// before the link counts as dead: two and a half of the gateway's 30 s pings.
/// A laptop that slept or a VPN that dropped without a reset leaves a socket
/// that never closes; without this the row would say *Connected* until the
/// TCP stack gave up, minutes later.
pub const SILENCE_LIMIT: Duration = Duration::from_secs(75);

/// How long a connected machine has to answer a ping sent after a wake from
/// sleep or a network change: a LAN answers in milliseconds, while an overlay
/// over a relay can take seconds on a live socket.
pub fn answer_deadline(route: &Route) -> Duration {
    match route.kind {
        crate::registry::RouteKind::Lan => Duration::from_secs(3),
        _ => Duration::from_secs(15),
    }
}

/// How much further the wall clock may run than the monotonic one during a
/// wait before the difference counts as sleep. Generous, so a clock that NTP
/// nudges does not read as a wake.
pub const SLEEP_GAP: Duration = Duration::from_secs(10);

/// Whether this computer slept during a wait that took `monotonic` by the
/// monotonic clock and `wall` by the wall clock. The monotonic clock stops
/// while macOS and Linux sleep and the wall clock does not, so a lid that was
/// closed shows up as the wall clock running ahead. A network that came back
/// with the lid is worth trying at once rather than at the end of the backoff.
pub fn slept(monotonic: Duration, wall: Duration) -> bool {
    wall > monotonic.saturating_add(SLEEP_GAP)
}

/// Why a paired machine is not connected, as a word an agent and the app can
/// branch on. Only [`Cause::Unreachable`] means no route answered; every other
/// cause is a machine that is up and said something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// No route answered: the machine is off, asleep, or on no network this
    /// computer reaches.
    Unreachable,
    /// The machine answered and refused this computer's pairing token: pair
    /// with it again. Not retried, because nothing changes until then.
    RejectedToken,
    /// The machine speaks another remote protocol version: update RustCode on
    /// one side.
    VersionMismatch,
    /// The machine let this computer in but not at its threads: its owner has
    /// to allow this device.
    NotAllowed,
    /// The machine's gateway answered, but no RustCode server runs behind it
    /// to relay to.
    ServerNotRunning,
    /// The machine answered and is busy, rate-limiting or restarting.
    Busy,
    /// The machine answered and refused for another reason.
    Rejected,
    /// The connection came up and then ended.
    Dropped,
}

impl Cause {
    /// The wire name `list_machines` and the app use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::RejectedToken => "rejected_token",
            Self::VersionMismatch => "version_mismatch",
            Self::NotAllowed => "not_allowed",
            Self::ServerNotRunning => "server_not_running",
            Self::Busy => "busy",
            Self::Rejected => "rejected",
            Self::Dropped => "dropped",
        }
    }

    /// Whether a route answered: the machine is on, and the problem is what
    /// it said rather than whether it is there.
    pub fn reachable(self) -> bool {
        !matches!(self, Self::Unreachable | Self::Dropped)
    }

    /// The cause a gateway error code stands for.
    pub fn of_code(code: ErrorCode) -> Self {
        match code {
            code if code.needs_new_pairing() => Self::RejectedToken,
            ErrorCode::UnsupportedVersion => Self::VersionMismatch,
            ErrorCode::UnauthorizedAccess => Self::NotAllowed,
            ErrorCode::Internal => Self::ServerNotRunning,
            ErrorCode::Busy
            | ErrorCode::ConnectBusy
            | ErrorCode::RateLimited
            | ErrorCode::ConnectRateLimited
            | ErrorCode::ShuttingDown => Self::Busy,
            _ => Self::Rejected,
        }
    }

    /// Why an attempt failed, from its typed error, never its message. A
    /// refusal names its code ([`Refusal`], or [`Unreachable::refused`] when
    /// every route failed and one that proved it is the machine refused); a
    /// saved token that is gone or belongs to a replaced device record
    /// ([`PairingLost`]) is a token problem; another computer on a saved
    /// address ([`NotThisMachine`]) and anything else did not answer.
    pub fn of(error: &anyhow::Error) -> Self {
        if let Some(refusal) = error.downcast_ref::<Refusal>() {
            return Self::of_code(refusal.code);
        }
        if let Some(code) = error
            .downcast_ref::<Unreachable>()
            .and_then(|unreachable| unreachable.refused)
        {
            return Self::of_code(code);
        }
        if error.downcast_ref::<PairingLost>().is_some() {
            return Self::RejectedToken;
        }
        if error.downcast_ref::<NotThisMachine>().is_some() {
            return Self::Unreachable;
        }
        Self::Unreachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{OverlayProvider, RouteKind};

    #[test]
    fn a_lan_answers_a_ping_sooner_than_an_overlay() {
        assert_eq!(
            answer_deadline(&Route::with_kind(RouteKind::Lan, "10.0.0.31:17879")),
            Duration::from_secs(3)
        );
        assert_eq!(
            answer_deadline(&Route::with_kind(RouteKind::Vpn, "100.90.27.158:17879")),
            Duration::from_secs(15)
        );
        // Silence is judged against the gateway's 30 s ping, with room for two
        // lost ones.
        assert!(SILENCE_LIMIT > Duration::from_secs(60));
    }

    #[test]
    fn a_wall_clock_that_ran_ahead_is_a_wake_from_sleep() {
        let five = Duration::from_secs(5);
        assert!(!slept(five, five));
        // NTP nudging the clock is not sleep.
        assert!(!slept(five, five + Duration::from_secs(3)));
        // A lid closed for an hour during a five-second slice.
        assert!(slept(five, Duration::from_secs(3_605)));
        // A wall clock set back is not sleep either.
        assert!(!slept(five, Duration::ZERO));
    }

    #[test]
    fn a_machine_that_answered_is_told_apart_from_one_that_did_not() {
        let refusal = |code| {
            anyhow::Error::new(Refusal {
                step: "acp_open".to_owned(),
                code,
                message: "the relay to this machine's engine could not be opened".to_owned(),
                retry_after_secs: None,
            })
        };
        // What a gateway whose server died says (Lagos, 2026-11-12).
        let no_server = Cause::of(&refusal(ErrorCode::Internal));
        assert_eq!(no_server, Cause::ServerNotRunning);
        assert!(no_server.reachable());
        assert_eq!(
            Cause::of(&refusal(ErrorCode::UnsupportedVersion)),
            Cause::VersionMismatch
        );
        assert_eq!(
            Cause::of(&refusal(ErrorCode::UnauthorizedAccess)),
            Cause::NotAllowed
        );
        assert_eq!(
            Cause::of(&refusal(ErrorCode::Unauthorized)),
            Cause::RejectedToken
        );
        assert_eq!(Cause::of(&refusal(ErrorCode::Busy)), Cause::Busy);
        // A context on top does not hide the refusal.
        assert_eq!(
            Cause::of(&refusal(ErrorCode::Internal).context("open ACP")),
            Cause::ServerNotRunning
        );

        // Every route failed: one that refused decides, else none answered.
        let lost = |refused| {
            anyhow::Error::new(Unreachable {
                machine: "Lagos".to_owned(),
                attempts: vec!["10.0.0.31:17879: timed out".to_owned()],
                refused,
                answered: refused.map(|_| "100.90.27.158:17879".to_owned()),
            })
        };
        assert_eq!(Cause::of(&lost(None)), Cause::Unreachable);
        assert!(!Cause::Unreachable.reachable());
        assert_eq!(
            Cause::of(&lost(Some(ErrorCode::UnsupportedVersion))),
            Cause::VersionMismatch
        );
        assert_eq!(
            Cause::of(&lost(Some(ErrorCode::TokenReplayed))),
            Cause::RejectedToken
        );

        // The token lookup and the identity check report typed errors; the
        // same words without the type are not a token problem.
        let lost = anyhow::Error::new(PairingLost {
            machine: "Lagos".to_owned(),
            reason: crate::machine::PairingLostReason::NoToken,
        })
        .context("connecting to Lagos");
        assert_eq!(Cause::of(&lost), Cause::RejectedToken);
        assert_eq!(
            Cause::of(&anyhow::anyhow!(
                "Lagos has no pairing token; pair with it again"
            )),
            Cause::Unreachable
        );
        let stranger = anyhow::Error::new(NotThisMachine {
            address: "10.0.0.31:17879".to_owned(),
            machine: "Lagos".to_owned(),
            answered: Some("Office PC".to_owned()),
        });
        assert_eq!(Cause::of(&stranger), Cause::Unreachable);
        assert_eq!(
            Cause::of(&anyhow::anyhow!("connection refused")),
            Cause::Unreachable
        );
        // A race lost because the token is gone carries it as `unauthorized`
        // from the route that proved it is the machine.
        let tokenless = anyhow::Error::new(Unreachable {
            machine: "Lagos".to_owned(),
            attempts: vec!["100.90.27.158:17879: Lagos has no pairing token".into()],
            refused: Some(ErrorCode::Unauthorized),
            answered: Some("100.90.27.158:17879".into()),
        });
        assert_eq!(Cause::of(&tokenless), Cause::RejectedToken);
        assert_eq!(Cause::RejectedToken.as_str(), "rejected_token");
        assert_eq!(Cause::ServerNotRunning.as_str(), "server_not_running");
    }

    #[test]
    fn the_backoff_doubles_to_a_minute_and_stays_there() {
        assert_eq!(
            (0..8)
                .map(|attempt| backoff_cap(attempt).as_secs())
                .collect::<Vec<_>>(),
            [1, 2, 4, 8, 16, 32, 60, 60]
        );
        // A long-lived outage does not overflow and does not grow past the cap.
        for attempt in 1..40 {
            assert!(backoff_cap(attempt) >= backoff_cap(attempt - 1));
        }
        assert_eq!(backoff_cap(u32::MAX), BACKOFF_MAX);
    }

    #[test]
    fn the_delay_stays_between_half_and_all_of_the_cap() {
        for attempt in 0..10 {
            let cap = backoff_cap(attempt);
            assert_eq!(backoff_delay(attempt, 0.0), cap / 2);
            assert_eq!(backoff_delay(attempt, 1.0), cap);
            for step in 0..=10 {
                let delay = backoff_delay(attempt, f64::from(step) / 10.0);
                assert!(
                    delay >= cap / 2 && delay <= cap,
                    "{attempt}/{step}: {delay:?}"
                );
            }
        }
        // A jitter outside 0..1 is clamped, not trusted.
        assert_eq!(backoff_delay(3, -1.0), backoff_delay(3, 0.0));
        assert_eq!(backoff_delay(3, 4.0), backoff_delay(3, 1.0));
    }

    #[test]
    fn the_route_that_worked_last_is_tried_first_within_its_class() {
        let lan = Route::with_kind(RouteKind::Lan, "192.168.1.20:17879");
        let other_lan = Route::with_kind(RouteKind::Lan, "10.0.0.9:17879");
        let mut netbird = Route::with_kind(RouteKind::Vpn, "100.90.16.237:17879");
        netbird.provider = Some(OverlayProvider::Netbird);
        let public = Route::with_kind(RouteKind::Public, "example.com:17879");
        let routes = [
            lan.clone(),
            other_lan.clone(),
            netbird.clone(),
            public.clone(),
        ];
        assert_eq!(
            prefer_last_working(&routes, Some("10.0.0.9:17879")),
            [
                other_lan.clone(),
                lan.clone(),
                netbird.clone(),
                public.clone()
            ]
        );
        // An overlay that worked last still goes after the LAN routes: coming
        // home must not dial the VPN first.
        assert_eq!(
            prefer_last_working(&routes, Some("100.90.16.237:17879")),
            [
                lan.clone(),
                other_lan.clone(),
                netbird.clone(),
                public.clone()
            ]
        );
        // Nothing worked last, or the route is gone: the saved order stands.
        assert_eq!(prefer_last_working(&routes, None), routes);
        assert_eq!(prefer_last_working(&routes, Some("nowhere:17879")), routes);
    }

    #[test]
    fn a_new_address_is_a_change_and_an_empty_list_is_not() {
        let here: Vec<IpAddr> = ["192.168.1.20", "100.92.13.44"]
            .iter()
            .map(|address| address.parse().unwrap())
            .collect();
        let same_again: Vec<IpAddr> = ["100.92.13.44", "192.168.1.20"]
            .iter()
            .map(|address| address.parse().unwrap())
            .collect();
        assert!(
            !network_changed(&here, &same_again),
            "the order is not a change"
        );
        // The VPN came up: a new address.
        let mut with_vpn = same_again.clone();
        with_vpn.push("fdfd::9".parse().unwrap());
        assert!(network_changed(&here, &with_vpn));
        // The laptop joined another Wi-Fi.
        let moved: Vec<IpAddr> = ["192.168.31.185"]
            .iter()
            .map(|address| address.parse().unwrap())
            .collect();
        assert!(network_changed(&here, &moved));
        // A machine with no readable interfaces must not retry in a loop.
        assert!(!network_changed(&[], &moved));
        assert!(!network_changed(&here, &[]));
        assert!(!network_changed(&[], &[]));
    }
}
