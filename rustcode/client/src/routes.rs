//! Route ordering, racing, and the hint a route that does not answer earns.
//!
//! A machine is one entry with several routes: the LAN at home, an overlay
//! such as NetBird or Tailscale away from it, and whatever was learned from
//! the machine's handshake. A connection starts every LAN route at once,
//! gives them [`LAN_GRACE`] on their own, then starts the overlay routes, and
//! keeps the first that authenticates as the machine. All of it ends inside
//! [`RACE_DEADLINE`], so an attempt never hangs on an address that is not
//! there.
//!
//! Nothing here opens a socket: [`race`] takes the dialer as a closure, and
//! the deadlines are tokio timers, which a unit test pauses and advances.
//! Moving between routes is the same race run again, so a connection that
//! drops re-orders itself without any extra state.

use crate::protocol::ErrorCode;
use crate::registry::{OverlayProvider, Route, RouteKind};
use crate::transport::port_or_default;
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use tokio::time::Instant;

/// How long the LAN routes are tried on their own before the overlay routes
/// join: enough for a home network to answer, short enough that the overlay
/// is not kept waiting.
pub const LAN_GRACE: Duration = Duration::from_secs(1);

/// The whole race. Past this, no route is still worth the wait.
pub const RACE_DEADLINE: Duration = Duration::from_secs(5);

/// How a race spends its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RaceSettings {
    /// LAN routes get this long on their own before the overlay routes join.
    pub lan_grace: Duration,
    /// From the first attempt to the answer.
    pub deadline: Duration,
}

impl Default for RaceSettings {
    fn default() -> Self {
        Self {
            lan_grace: LAN_GRACE,
            deadline: RACE_DEADLINE,
        }
    }
}

/// One route that did not win, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFailure {
    pub route: Route,
    pub reason: String,
    /// The machine's refusal of this route's credential, when it answered.
    pub refusal: Option<ErrorCode>,
}

/// What one dial of one route may say. A refusal is the machine's own answer
/// (a revoked pairing, a missing ACP grant) and is kept apart from a route
/// that simply did not answer.
#[derive(Debug, Clone)]
pub struct DialFailure {
    pub reason: String,
    pub refusal: Option<ErrorCode>,
}

impl DialFailure {
    /// A route that did not answer, or answered as something else.
    pub fn unreachable(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            refusal: None,
        }
    }

    pub fn refused(reason: impl Into<String>, code: ErrorCode) -> Self {
        Self {
            reason: reason.into(),
            refusal: Some(code),
        }
    }
}

impl From<String> for DialFailure {
    fn from(reason: String) -> Self {
        Self::unreachable(reason)
    }
}

impl From<&str> for DialFailure {
    fn from(reason: &str) -> Self {
        Self::unreachable(reason)
    }
}

/// The outcome of a race: a winner with the failures that came before it, or
/// only the failures.
#[derive(Debug)]
pub enum Race<T> {
    Won {
        value: T,
        route: Route,
        failures: Vec<RouteFailure>,
    },
    Lost {
        failures: Vec<RouteFailure>,
    },
}

/// Routes in the order they are raced: LAN first, then the overlay routes,
/// then a public one. Stable, so the saved order inside a class is kept.
pub fn ordered(routes: &[Route]) -> Vec<Route> {
    let mut ordered = routes.to_vec();
    ordered.sort_by_key(|route| route.kind.rank());
    ordered
}

type DialFuture<'a, T> = Pin<Box<dyn Future<Output = (Route, Result<T, DialFailure>)> + Send + 'a>>;

fn dial_one<'a, T, F, Fut>(dial: &'a F, route: Route) -> DialFuture<'a, T>
where
    F: Fn(Route) -> Fut + Sync,
    Fut: Future<Output = Result<T, DialFailure>> + Send + 'a,
{
    Box::pin(async move {
        let result = dial(route.clone()).await;
        (route, result)
    })
}

/// Race the routes and return the first that dials, within the settings.
///
/// LAN routes start together. When there is no LAN route, or none answers
/// inside [`RaceSettings::lan_grace`], the overlay routes start too. A public
/// route is never raced: the caller refuses it before a credential travels.
pub async fn race<T, F, Fut>(routes: &[Route], settings: &RaceSettings, dial: F) -> Race<T>
where
    F: Fn(Route) -> Fut + Sync,
    Fut: Future<Output = Result<T, DialFailure>> + Send,
{
    let ordered = ordered(routes);
    let mut lan: VecDeque<Route> = ordered
        .iter()
        .filter(|route| route.kind == RouteKind::Lan)
        .cloned()
        .collect();
    let mut overlay: VecDeque<Route> = ordered
        .iter()
        .filter(|route| route.kind != RouteKind::Lan && route.kind != RouteKind::Public)
        .cloned()
        .collect();

    let had_lan = !lan.is_empty();
    let start = Instant::now();
    let deadline = start + settings.deadline;
    let overlay_due = start + settings.lan_grace;
    let mut overlay_started = !had_lan;
    let mut failures: Vec<RouteFailure> = Vec::new();
    let mut in_flight: FuturesUnordered<DialFuture<'_, T>> = FuturesUnordered::new();

    for route in lan.drain(..) {
        in_flight.push(dial_one(&dial, route));
    }
    if overlay_started {
        for route in overlay.drain(..) {
            in_flight.push(dial_one(&dial, route));
        }
    }

    loop {
        if in_flight.is_empty() && (overlay_started || overlay.is_empty()) {
            break;
        }
        let now = Instant::now();
        let wake = if overlay_started {
            deadline
        } else {
            overlay_due.min(deadline)
        };
        if now >= wake {
            if !overlay_started {
                overlay_started = true;
                for route in overlay.drain(..) {
                    in_flight.push(dial_one(&dial, route));
                }
                continue;
            }
            break;
        }
        tokio::select! {
            biased;
            Some((route, result)) = in_flight.next() => match result {
                Ok(value) => {
                    // Dropping the other attempts cancels them: a LAN route
                    // that answered makes the overlay attempt pointless.
                    return Race::Won { value, route, failures };
                }
                Err(failure) => failures.push(RouteFailure {
                    route,
                    reason: failure.reason,
                    refusal: failure.refusal,
                }),
            },
            _ = tokio::time::sleep_until(wake) => {
                if !overlay_started {
                    overlay_started = true;
                    for route in overlay.drain(..) {
                        in_flight.push(dial_one(&dial, route));
                    }
                } else {
                    break;
                }
            }
        }
    }
    Race::Lost { failures }
}

/// What a route that does not answer means when another route to the same
/// machine does answer. `None` when nothing failed on a different kind of
/// network: there is nothing to compare and nothing the user can act on.
///
/// This is the words `rustcode machines list` prints under a machine and the
/// app shows in its status, so the reason a machine is reachable one way and
/// not the other is on screen instead of being guessed at.
pub fn route_hint(in_use: &Route, failures: &[RouteFailure]) -> Option<String> {
    let overlay = |route: &Route| matches!(route.kind, RouteKind::Vpn | RouteKind::Tailscale);
    if !overlay(in_use) {
        let failure = failures.iter().find(|failure| overlay(&failure.route))?;
        return Some(overlay_hint(&failure.route));
    }
    let lan = failures
        .iter()
        .find(|failure| failure.route.kind == RouteKind::Lan)?;
    Some(format!(
        "Reachable over {name} but not on the LAN ({address}). That is expected away from home; \
         the LAN route stays saved and is tried first when both machines are on the same network.",
        name = in_use.network_name(),
        address = lan.route.address,
    ))
}

fn overlay_hint(route: &Route) -> String {
    let port = port_or_default(&route.address);
    let Some(provider) = route.provider else {
        return format!(
            "Not reachable over the VPN ({address}). On that machine, check that the VPN allows \
             inbound connections and that a firewall or access policy allows TCP {port} between \
             the two machines.",
            address = route.address,
        );
    };
    let name = provider.display_name();
    // Each product has its own control for this. Naming NetBird's inbound
    // toggle on Tailscale would send the user looking for a setting that does
    // not exist there.
    let check = match provider {
        OverlayProvider::Netbird => format!(
            "check that NetBird allows inbound connections and that an access policy allows TCP \
             {port} between the two machines"
        ),
        OverlayProvider::Tailscale => {
            format!("check that Tailscale's ACLs or grants allow TCP {port} to that machine")
        }
        OverlayProvider::Zerotier => {
            format!("check that ZeroTier's flow rules allow TCP {port} to that machine")
        }
    };
    format!(
        "Not reachable over {name} ({address}). On that machine, {check}.",
        address = route.address,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lan(address: &str) -> Route {
        Route::with_kind(RouteKind::Lan, address)
    }

    fn overlay(address: &str, provider: Option<OverlayProvider>) -> Route {
        let mut route = Route::with_kind(RouteKind::Vpn, address);
        route.provider = provider;
        route
    }

    type Answer = (Duration, Result<&'static str, DialFailure>);

    /// A dialer that answers per address after a latency. An address the plan
    /// does not name never answers.
    fn dialer(
        plan: HashMap<String, Answer>,
    ) -> impl Fn(Route) -> Pin<Box<dyn Future<Output = Result<&'static str, DialFailure>> + Send>> + Clone
    {
        move |route: Route| {
            let plan = plan.clone();
            Box::pin(async move {
                let (latency, answer) = plan.get(&route.address).cloned().unwrap_or((
                    Duration::from_secs(60),
                    Err(DialFailure::unreachable("timeout")),
                ));
                tokio::time::sleep(latency).await;
                answer
            })
        }
    }

    fn settings() -> RaceSettings {
        RaceSettings::default()
    }

    #[tokio::test(start_paused = true)]
    async fn a_lan_route_that_answers_wins_before_the_overlay_starts() {
        let routes = [
            lan("192.168.1.20:17879"),
            overlay("100.90.16.237:17879", None),
        ];
        let start = Instant::now();
        let outcome = race(
            &routes,
            &settings(),
            dialer(HashMap::from([
                (
                    "192.168.1.20:17879".into(),
                    (Duration::from_millis(20), Ok("lan")),
                ),
                (
                    "100.90.16.237:17879".into(),
                    (Duration::from_millis(10), Ok("overlay")),
                ),
            ])),
        )
        .await;
        assert!(start.elapsed() < LAN_GRACE, "the LAN answered first");
        match outcome {
            Race::Won {
                value,
                route,
                failures,
            } => {
                assert_eq!(value, "lan");
                assert_eq!(route.address, "192.168.1.20:17879");
                assert!(failures.is_empty(), "the overlay was never tried");
            }
            other => panic!("expected a LAN win, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn the_overlay_joins_when_the_lan_has_not_answered() {
        let routes = [
            lan("192.168.1.20:17879"),
            overlay("100.90.16.237:17879", None),
        ];
        let start = Instant::now();
        let outcome = race(
            &routes,
            &settings(),
            dialer(HashMap::from([
                (
                    "192.168.1.20:17879".into(),
                    (
                        Duration::from_millis(10),
                        Err(DialFailure::unreachable("no route to host")),
                    ),
                ),
                (
                    "100.90.16.237:17879".into(),
                    (Duration::from_millis(50), Ok("overlay")),
                ),
            ])),
        )
        .await;
        let elapsed = start.elapsed();
        assert!(
            elapsed >= LAN_GRACE,
            "the overlay waited for the grace: {elapsed:?}"
        );
        assert!(elapsed < RACE_DEADLINE, "and answered inside the deadline");
        match outcome {
            Race::Won {
                value,
                route,
                failures,
            } => {
                assert_eq!(value, "overlay");
                assert_eq!(route.address, "100.90.16.237:17879");
                assert_eq!(failures.len(), 1);
                assert_eq!(failures[0].route.address, "192.168.1.20:17879");
            }
            other => panic!("expected an overlay win, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn without_a_lan_route_the_overlay_starts_at_once() {
        let routes = [overlay("100.90.16.237:17879", None)];
        let start = Instant::now();
        let outcome = race(
            &routes,
            &settings(),
            dialer(HashMap::from([(
                "100.90.16.237:17879".into(),
                (Duration::from_millis(10), Ok("overlay")),
            )])),
        )
        .await;
        assert!(start.elapsed() < LAN_GRACE);
        assert!(matches!(outcome, Race::Won { .. }));
    }

    #[tokio::test(start_paused = true)]
    async fn every_route_failing_ends_at_the_deadline_with_each_reason() {
        let routes = [
            lan("192.168.1.20:17879"),
            overlay("100.90.16.237:17879", Some(OverlayProvider::Netbird)),
        ];
        let start = Instant::now();
        let outcome = race(
            &routes,
            &settings(),
            dialer(HashMap::from([
                (
                    "192.168.1.20:17879".into(),
                    (
                        Duration::from_millis(10),
                        Err(DialFailure::unreachable("refused")),
                    ),
                ),
                (
                    "100.90.16.237:17879".into(),
                    (
                        Duration::from_secs(30),
                        Err(DialFailure::unreachable("too slow")),
                    ),
                ),
            ])),
        )
        .await;
        assert!(start.elapsed() >= RACE_DEADLINE);
        match outcome {
            Race::Lost { failures } => {
                assert_eq!(
                    failures.len(),
                    1,
                    "the overlay attempt was still open: {failures:?}"
                );
                assert_eq!(failures[0].reason, "refused");
            }
            other => panic!("expected every route to fail, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_lan_route_still_beats_a_later_overlay_route() {
        let routes = [
            lan("192.168.1.20:17879"),
            overlay("100.90.16.237:17879", None),
        ];
        let outcome = race(
            &routes,
            &settings(),
            dialer(HashMap::from([
                (
                    "192.168.1.20:17879".into(),
                    (Duration::from_millis(400), Ok("lan")),
                ),
                (
                    "100.90.16.237:17879".into(),
                    (Duration::from_millis(200), Ok("overlay")),
                ),
            ])),
        )
        .await;
        match outcome {
            Race::Won { value, .. } => assert_eq!(value, "lan", "the LAN started first"),
            other => panic!("expected a LAN win, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_refusal_keeps_its_code_for_the_caller() {
        let routes = [overlay("100.90.16.237:17879", None)];
        let outcome = race(
            &routes,
            &settings(),
            dialer(HashMap::from([(
                "100.90.16.237:17879".into(),
                (
                    Duration::from_millis(5),
                    Err(DialFailure::refused("revoked", ErrorCode::Revoked)),
                ),
            )])),
        )
        .await;
        match outcome {
            Race::Lost { failures } => {
                assert_eq!(failures[0].refusal, Some(ErrorCode::Revoked));
            }
            other => panic!("expected a loss, got {other:?}"),
        }
    }

    #[test]
    fn the_race_order_puts_the_lan_first_and_public_last() {
        let routes = [
            Route::with_kind(RouteKind::Public, "example.com:17879"),
            Route::with_kind(RouteKind::Tailscale, "mac.tail1234.ts.net:17879"),
            lan("192.168.1.20:17879"),
            overlay("100.90.16.237:17879", None),
            lan("10.0.0.9:17879"),
        ];
        let ordered = ordered(&routes);
        assert_eq!(
            ordered
                .iter()
                .map(|route| route.address.as_str())
                .collect::<Vec<_>>(),
            [
                "192.168.1.20:17879",
                "10.0.0.9:17879",
                "mac.tail1234.ts.net:17879",
                "100.90.16.237:17879",
                "example.com:17879",
            ]
        );
    }

    #[test]
    fn an_overlay_that_fails_while_the_lan_works_says_what_to_check() {
        let route = overlay("100.90.16.237:17879", Some(OverlayProvider::Netbird));
        let hint = route_hint(
            &lan("192.168.1.20:17879"),
            &[RouteFailure {
                route,
                reason: "connection timed out".into(),
                refusal: None,
            }],
        )
        .expect("a hint");
        assert_eq!(
            hint,
            "Not reachable over NetBird (100.90.16.237:17879). On that machine, check that \
             NetBird allows inbound connections and that an access policy allows TCP 17879 \
             between the two machines."
        );
    }

    #[test]
    fn a_tailscale_hint_names_tailscales_own_control() {
        let hint = route_hint(
            &lan("192.168.1.20:17879"),
            &[RouteFailure {
                route: overlay("100.101.1.2:17879", Some(OverlayProvider::Tailscale)),
                reason: "timed out".into(),
                refusal: None,
            }],
        )
        .expect("a hint");
        assert!(
            hint.contains("Tailscale's ACLs or grants allow TCP 17879"),
            "{hint}"
        );
        assert!(
            !hint.contains("inbound connections"),
            "NetBird's toggle does not exist on Tailscale: {hint}"
        );
    }

    #[test]
    fn a_zerotier_hint_names_flow_rules() {
        let hint = route_hint(
            &lan("192.168.1.20:17879"),
            &[RouteFailure {
                route: overlay("10.147.20.5:17879", Some(OverlayProvider::Zerotier)),
                reason: "timed out".into(),
                refusal: None,
            }],
        )
        .expect("a hint");
        assert!(hint.contains("ZeroTier's flow rules"), "{hint}");
    }

    #[test]
    fn an_overlay_without_a_provider_still_names_what_to_check() {
        let hint = route_hint(
            &lan("192.168.1.20:17879"),
            &[RouteFailure {
                route: overlay("100.90.16.237:17879", None),
                reason: "timed out".into(),
                refusal: None,
            }],
        )
        .expect("a hint");
        assert!(
            hint.starts_with("Not reachable over the VPN (100.90.16.237:17879)."),
            "{hint}"
        );
        assert!(hint.contains("inbound connections"), "{hint}");
    }

    #[test]
    fn a_lan_that_fails_while_the_overlay_works_reads_as_expected() {
        let hint = route_hint(
            &overlay("100.90.16.237:17879", Some(OverlayProvider::Tailscale)),
            &[RouteFailure {
                route: lan("192.168.1.20:17879"),
                reason: "no route to host".into(),
                refusal: None,
            }],
        )
        .expect("a hint");
        assert!(hint.contains("Reachable over Tailscale"), "{hint}");
        assert!(hint.contains("192.168.1.20:17879"), "{hint}");
    }

    #[test]
    fn only_a_different_kind_of_network_earns_a_hint() {
        let in_use = lan("192.168.1.20:17879");
        assert!(
            route_hint(
                &in_use,
                &[RouteFailure {
                    route: lan("10.0.0.9:17879"),
                    reason: "refused".into(),
                    refusal: None,
                }]
            )
            .is_none(),
            "two LAN routes say nothing about a VPN"
        );
        assert!(
            route_hint(&in_use, &[]).is_none(),
            "no failures, nothing to say"
        );
    }
}
