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
//!
//! One thing outside the race decides what a route is worth: whether this
//! computer is on the overlay the route is on at all. A machine that runs
//! Radmin VPN and NetBird reports both, and a computer with only NetBird can
//! never reach the Radmin address, so that route is dialled last and a hint is
//! never printed asking its owner to fix a VPN this computer is not on
//! ([`on_local_overlay`], [`ordered_for_this_computer`], [`RouteReport`]).

use crate::protocol::ErrorCode;
use crate::registry::{OverlayProvider, Route, RouteKind};
use crate::transport::{host_part, port_or_default};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use std::collections::VecDeque;
use std::future::Future;
use std::net::IpAddr;
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

/// The address space an address is in, which is how a route's overlay is
/// compared with this computer's own interfaces.
///
/// Meshes do not announce themselves in the routing table and macOS names
/// every VPN `utunN`, so the address is what is left to compare: two addresses
/// in the same space are on the same kind of overlay, and an address in a
/// space this computer has no interface in is on an overlay it is not on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpace {
    /// `100.64.0.0/10`, the shared address space NetBird and Tailscale hand
    /// out. One space for all of them: only a machine's own address range
    /// tells those products apart, and it does not matter here.
    SharedV4,
    /// A private, loopback or link-local IPv4 range, keyed by its first two
    /// octets: `10.8.0.2` (a WireGuard tunnel) and `10.147.20.5` (a ZeroTier
    /// network) are different spaces, and this computer's own address on the
    /// same ZeroTier network is the same one.
    PrivateV4(u8, u8),
    /// IPv6 unique-local (`fc00::/7`), keyed by its first two octets, which is
    /// what a VPN hands out: Tailscale's `fd7a:115c:a1e0::/48` is one space and
    /// Radmin's `fdfd::` another.
    UniqueLocalV6(u8, u8),
    /// IPv6 loopback and link-local.
    LocalV6,
    /// A globally routed IPv4 address.
    GlobalV4,
    /// A globally routed IPv6 address.
    GlobalV6,
}

/// The space `address` is in (see [`AddressSpace`]).
pub fn address_space(address: IpAddr) -> AddressSpace {
    match address {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            if first == 100 && (64..128).contains(&second) {
                AddressSpace::SharedV4
            } else if ip.is_loopback() || ip.is_private() || ip.is_link_local() {
                AddressSpace::PrivateV4(first, second)
            } else {
                AddressSpace::GlobalV4
            }
        }
        IpAddr::V6(ip) => {
            let octets = ip.octets();
            if (octets[0] & 0xfe) == 0xfc {
                AddressSpace::UniqueLocalV6(octets[0], octets[1])
            } else if ip.is_loopback() || ip.is_unicast_link_local() {
                AddressSpace::LocalV6
            } else {
                AddressSpace::GlobalV6
            }
        }
    }
}

/// This computer's own interface addresses, for telling an overlay this
/// computer is on from one it is not.
///
/// Loopback and link-local addresses are left out: neither is on an overlay,
/// and a link-local one needs a scope a route does not carry. A machine that
/// cannot read its interfaces gets an empty list, which shows no hint about a
/// VPN and tries its overlay routes last instead of dropping them.
pub fn local_addresses() -> Vec<IpAddr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };
    interfaces
        .into_iter()
        .filter(|interface| interface.is_oper_up() && !interface.is_loopback())
        .map(|interface| interface.ip())
        .filter(|address| {
            !address.is_loopback()
                && !address.is_unspecified()
                && !match address {
                    IpAddr::V4(ip) => ip.is_link_local(),
                    IpAddr::V6(ip) => ip.is_unicast_link_local(),
                }
        })
        .collect()
}

/// Whether `route` is on an overlay this computer has an interface on.
///
/// `true` for every route that is not on an overlay — the question means
/// nothing for a LAN or public route — and for an overlay route whose address
/// is a name rather than an address: a `.ts.net` name is resolved by the
/// overlay's own resolver, so it never answers from a space this computer is
/// not on the way a saved raw address does.
///
/// `local` is [`local_addresses`], passed in so the answer is a pure function
/// of what the caller read and a unit test can state both sides.
pub fn on_local_overlay(route: &Route, local: &[IpAddr]) -> bool {
    let Some(space) = overlay_space(route) else {
        return true;
    };
    local.iter().any(|address| address_space(*address) == space)
}

/// The address space an overlay route is in, or `None` for a route that is not
/// on an overlay and for a name that does not say.
fn overlay_space(route: &Route) -> Option<AddressSpace> {
    if !matches!(route.kind, RouteKind::Vpn | RouteKind::Tailscale) {
        return None;
    }
    host_part(&route.address)
        .parse::<IpAddr>()
        .ok()
        .map(address_space)
}

/// [`ordered`] with the routes on an overlay this computer is not on at the
/// back of their class: they cannot answer, so they are dialled only after
/// every route that can. Stable, so the order inside each group is kept, and a
/// public route stays last where [`ordered`] puts it.
pub fn ordered_for_this_computer(routes: &[Route], local: &[IpAddr]) -> Vec<Route> {
    let mut ordered = ordered(routes);
    ordered.sort_by_key(|route| (route.kind.rank(), !on_local_overlay(route, local)));
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

/// What a caller knows about a machine's routes when it asks for the hint to
/// show under its row.
///
/// This is the words `rustcode machines list` prints under a machine and the
/// app shows in its status, so the reason a machine is reachable one way and
/// not the other is on screen instead of being guessed at.
pub struct RouteReport<'a> {
    /// Every route saved for the machine.
    pub routes: &'a [Route],
    /// The route that answered.
    pub in_use: &'a Route,
    /// The routes that did not answer: the race's first, then the probe's.
    pub failures: &'a [RouteFailure],
    /// This computer's own interface addresses, from [`local_addresses`].
    pub local: &'a [IpAddr],
}

impl RouteReport<'_> {
    /// The hint for a route that does not answer while another one does, or
    /// `None` when there is nothing the owner can act on.
    ///
    /// An overlay route earns no hint when this computer has no interface on
    /// that overlay — it can never answer, so asking its owner to fix that VPN
    /// sends them looking for a setting on a network they are not on — or when
    /// another overlay route to the same machine answers, because then the
    /// machine is reachable over a VPN already (#3421).
    pub fn hint(&self) -> Option<String> {
        if is_overlay(self.in_use) {
            let lan = self
                .failures
                .iter()
                .find(|failure| failure.route.kind == RouteKind::Lan)?;
            return Some(format!(
                "Reachable over {name} but not on the LAN ({address}). That is expected away from \
                 home; the LAN route stays saved and is tried first when both machines are on the \
                 same network.",
                name = self.in_use.network_name(),
                address = lan.route.address,
            ));
        }
        let failure = self.failures.iter().find(|failure| {
            is_overlay(&failure.route) && on_local_overlay(&failure.route, self.local)
        })?;
        // Another overlay route to the machine is not among the failures, so it
        // answered (or is the one in use): the VPN between the two machines
        // works, and this one address is only a route that is not needed.
        let another_answers = self.routes.iter().any(|route| {
            is_overlay(route)
                && route.address != failure.route.address
                && !self
                    .failures
                    .iter()
                    .any(|failed| failed.route.address == route.address)
        });
        (!another_answers).then(|| overlay_hint(&failure.route))
    }
}

fn is_overlay(route: &Route) -> bool {
    matches!(route.kind, RouteKind::Vpn | RouteKind::Tailscale)
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

    /// The addresses this computer has, as `local_addresses` would report them.
    fn local(addresses: &[&str]) -> Vec<IpAddr> {
        addresses
            .iter()
            .map(|address| address.parse().expect("an address"))
            .collect()
    }

    fn failure(route: Route, reason: &str) -> RouteFailure {
        RouteFailure {
            route,
            reason: reason.into(),
            refusal: None,
        }
    }

    /// The hint for a machine whose routes are `routes`, connected over
    /// `in_use`, with `failures` that did not answer.
    fn hint_with(
        routes: &[Route],
        in_use: &Route,
        failures: &[RouteFailure],
        local: &[IpAddr],
    ) -> Option<String> {
        RouteReport {
            routes,
            in_use,
            failures,
            local,
        }
        .hint()
    }

    #[test]
    fn an_overlay_that_fails_while_the_lan_works_says_what_to_check() {
        let netbird = overlay("100.90.16.237:17879", Some(OverlayProvider::Netbird));
        let home = lan("192.168.1.20:17879");
        let hint = hint_with(
            &[home.clone(), netbird.clone()],
            &home,
            &[failure(netbird, "connection timed out")],
            &local(&["192.168.1.20", "100.92.13.44"]),
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
        let tailscale = overlay("100.101.1.2:17879", Some(OverlayProvider::Tailscale));
        let home = lan("192.168.1.20:17879");
        let hint = hint_with(
            &[home.clone(), tailscale.clone()],
            &home,
            &[failure(tailscale, "timed out")],
            &local(&["192.168.1.20", "100.101.1.9"]),
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
        let zerotier = overlay("10.147.20.5:17879", Some(OverlayProvider::Zerotier));
        let home = lan("192.168.1.20:17879");
        let hint = hint_with(
            &[home.clone(), zerotier.clone()],
            &home,
            &[failure(zerotier, "timed out")],
            &local(&["192.168.1.20", "10.147.20.9"]),
        )
        .expect("a hint");
        assert!(hint.contains("ZeroTier's flow rules"), "{hint}");
    }

    #[test]
    fn an_overlay_without_a_provider_still_names_what_to_check() {
        let vpn = overlay("100.90.16.237:17879", None);
        let home = lan("192.168.1.20:17879");
        let hint = hint_with(
            &[home.clone(), vpn.clone()],
            &home,
            &[failure(vpn, "timed out")],
            &local(&["192.168.1.20", "100.92.13.44"]),
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
        let netbird = overlay("100.90.16.237:17879", Some(OverlayProvider::Tailscale));
        let home = lan("192.168.1.20:17879");
        let hint = hint_with(
            &[home.clone(), netbird.clone()],
            &netbird,
            &[failure(home, "no route to host")],
            &local(&["192.168.1.20", "100.92.13.44"]),
        )
        .expect("a hint");
        assert!(hint.contains("Reachable over Tailscale"), "{hint}");
        assert!(hint.contains("192.168.1.20:17879"), "{hint}");
    }

    #[test]
    fn only_a_different_kind_of_network_earns_a_hint() {
        let home = lan("192.168.1.20:17879");
        let other = lan("10.0.0.9:17879");
        assert!(
            hint_with(
                &[home.clone(), other.clone()],
                &home,
                &[failure(other, "refused")],
                &local(&["192.168.1.20"]),
            )
            .is_none(),
            "two LAN routes say nothing about a VPN"
        );
        assert!(
            hint_with(&[home.clone()], &home, &[], &local(&["192.168.1.20"])).is_none(),
            "no failures, nothing to say"
        );
    }

    /// A machine that runs Radmin VPN as well as NetBird reports both, and
    /// this computer is on neither: asking its owner to fix a VPN this
    /// computer is not on sends them looking for a setting that is not theirs
    /// (#3421).
    #[test]
    fn a_route_on_an_overlay_this_computer_is_not_on_earns_no_hint() {
        let radmin = overlay("[fdfd::2]:17879", None);
        let home = lan("192.168.1.20:17879");
        let routes = [home.clone(), radmin.clone()];
        let failures = [failure(radmin.clone(), "connection timed out")];
        // This computer has the LAN and NetBird, not Radmin's IPv6 overlay.
        assert!(
            hint_with(
                &routes,
                &home,
                &failures,
                &local(&["192.168.1.20", "100.92.13.44"])
            )
            .is_none(),
            "no interface on that overlay, so no hint about it"
        );
        // On a computer that is on Radmin, the same failure is worth a hint.
        let hint = hint_with(
            &routes,
            &home,
            &failures,
            &local(&["192.168.1.20", "fdfd::9"]),
        )
        .expect("a hint");
        assert!(hint.contains("fdfd::2"), "{hint}");
    }

    /// A VPN route that fails matters only while no other VPN route to that
    /// machine answers: a machine reachable over NetBird needs no advice about
    /// its Radmin address (#3421).
    #[test]
    fn another_overlay_that_answers_silences_the_same_vpn_hint() {
        let home = lan("192.168.1.20:17879");
        let netbird = overlay("100.90.16.237:17879", Some(OverlayProvider::Netbird));
        let radmin = overlay("[fdfd::2]:17879", None);
        let routes = [home.clone(), netbird.clone(), radmin.clone()];
        let here = local(&["192.168.1.20", "100.92.13.44", "fdfd::9"]);
        // NetBird answered, so it is not among the failures.
        assert!(
            hint_with(
                &routes,
                &home,
                &[failure(radmin.clone(), "timed out")],
                &here
            )
            .is_none(),
            "the machine is reachable over NetBird already"
        );
        // With no other overlay route answering, the Radmin failure is the
        // one thing worth saying.
        let hint = hint_with(
            &[home.clone(), radmin.clone()],
            &home,
            &[failure(radmin, "timed out")],
            &here,
        )
        .expect("a hint");
        assert!(hint.contains("fdfd::2"), "{hint}");
    }

    #[test]
    fn an_address_space_is_what_tells_two_overlays_apart() {
        for (address, space) in [
            ("100.92.13.44", AddressSpace::SharedV4),
            ("100.64.0.1", AddressSpace::SharedV4),
            ("100.128.0.1", AddressSpace::GlobalV4),
            ("10.8.0.2", AddressSpace::PrivateV4(10, 8)),
            ("10.147.20.5", AddressSpace::PrivateV4(10, 147)),
            ("192.168.1.20", AddressSpace::PrivateV4(192, 168)),
            (
                "[fd7a:115c:a1e0::1]",
                AddressSpace::UniqueLocalV6(0xfd, 0x7a),
            ),
            ("[fdfd::2]", AddressSpace::UniqueLocalV6(0xfd, 0xfd)),
            ("[fe80::1]", AddressSpace::LocalV6),
            ("[2001:db8::20]", AddressSpace::GlobalV6),
            ("203.0.113.9", AddressSpace::GlobalV4),
        ] {
            let address: IpAddr = address.trim_matches(['[', ']']).parse().unwrap();
            assert_eq!(address_space(address), space, "{address}");
        }
    }

    /// Only an overlay route is on an overlay; a name never rules itself out,
    /// because the overlay's own resolver answers it.
    #[test]
    fn only_an_overlay_route_can_be_on_an_overlay_this_computer_is_not_on() {
        let here = local(&["192.168.1.20", "100.92.13.44"]);
        assert!(on_local_overlay(&lan("192.168.1.20:17879"), &here));
        assert!(
            on_local_overlay(&lan("10.9.9.9:17879"), &here),
            "a LAN route"
        );
        assert!(on_local_overlay(
            &overlay("100.90.16.237:17879", None),
            &here
        ));
        assert!(!on_local_overlay(&overlay("[fdfd::2]:17879", None), &here));
        assert!(!on_local_overlay(
            &overlay("10.147.20.5:17879", Some(OverlayProvider::Zerotier)),
            &here
        ));
        assert!(on_local_overlay(
            &Route::with_kind(RouteKind::Tailscale, "mac.tail1234.ts.net:17879"),
            &here
        ));
    }

    /// A route this computer cannot reach is dialled only after the ones it
    /// can, and the race's own order inside each group is kept.
    #[test]
    fn a_route_on_an_overlay_this_computer_is_not_on_is_tried_last() {
        let routes = [
            overlay("[fdfd::2]:17879", None),
            lan("192.168.1.20:17879"),
            overlay("100.90.16.237:17879", Some(OverlayProvider::Netbird)),
            Route::with_kind(RouteKind::Public, "example.com:17879"),
        ];
        let ordered = ordered_for_this_computer(&routes, &local(&["192.168.1.20", "100.92.13.44"]));
        assert_eq!(
            ordered
                .iter()
                .map(|route| route.address.as_str())
                .collect::<Vec<_>>(),
            [
                "192.168.1.20:17879",
                "100.90.16.237:17879",
                "[fdfd::2]:17879",
                "example.com:17879",
            ]
        );
        // Without any interface the list keeps the plain order: guessing that
        // nothing is reachable would be worse than trying.
        let none = ordered_for_this_computer(&routes, &[]);
        assert_eq!(
            none.iter()
                .map(|route| route.address.as_str())
                .collect::<Vec<_>>(),
            [
                "192.168.1.20:17879",
                "[fdfd::2]:17879",
                "100.90.16.237:17879",
                "example.com:17879",
            ]
        );
    }
}
