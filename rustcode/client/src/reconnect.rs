//! What a client does while a paired machine is away.
//!
//! A machine that sleeps, restarts or changes network comes back on its own:
//! the supervisor tries again, waiting a little longer after each failure but
//! never more than [`BACKOFF_MAX`], and it ends a wait early when this
//! computer's own network changes — the VPN came up, the laptop joined
//! another Wi-Fi — because that is what makes a route that was dead work
//! (#3436).
//!
//! Everything here is a pure function of its arguments, so the schedule, the
//! route order and "did the network change" are unit-tested rather than timed.

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{OverlayProvider, RouteKind};

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
