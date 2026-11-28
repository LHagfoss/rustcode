//! The relay a hub runs for its fleet ([ADR 0013](../../../../../docs/adr/0013-machine-relay-and-nat-traversal.md),
//! `docs/remote.md` › The hub). With `relay` on in `hub.json`, the hub's
//! gateway also listens as a relay on [`DEFAULT_PORT`] and hands its join
//! link to every member over the hub link, so each member registers with it
//! without a command of its own.

use super::super::address::{HostRoute, HostRouteKind};
use super::super::hub_link::HubConfig;
use super::Shared;
use crate::remote_relay::protocol::JoinLink;
use crate::remote_relay::server::Relay;
use crate::remote_relay::{DEFAULT_PORT, open};
use anyhow::{Context, Result};
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

/// How often the hub rereads its role and its routes.
const CHECK: Duration = Duration::from_secs(5);

/// Run the hub's relay while the role asks for it, until the gateway shuts
/// down.
pub(super) async fn run(shared: Arc<Shared>) {
    let config_dir = shared.lifecycle.config_directory();
    let mut running: Option<Arc<Relay>> = None;
    let mut last_error: Option<String> = None;
    let mut tick = tokio::time::interval(CHECK);
    loop {
        tokio::select! {
            _ = shared.shutdown.cancelled() => break,
            _ = tick.tick() => {}
        }
        let config = HubConfig::load(&config_dir);
        if !(config.hub && config.relay) {
            if let Some(relay) = running.take() {
                relay.shutdown_token().cancel();
                crate::dbg_log!("hub relay: stopped");
            }
        } else if running.is_none() {
            match start(&config_dir).await {
                Ok(relay) => {
                    crate::dbg_log!("hub relay: listening on port {DEFAULT_PORT}");
                    last_error = None;
                    running = Some(relay);
                }
                Err(error) => {
                    let error = format!("{error:#}");
                    if last_error.as_ref() != Some(&error) {
                        crate::dbg_log!("hub relay: {error}");
                        last_error = Some(error);
                    }
                }
            }
        }
        let link = running
            .as_ref()
            .and_then(|relay| join_link(relay, &config, &super::routes_now(&shared)));
        shared.members.set_relay_link(link);
    }
    if let Some(relay) = running {
        relay.shutdown_token().cancel();
    }
    shared.members.set_relay_link(None);
}

async fn start(config_dir: &Path) -> Result<Arc<Relay>> {
    let relay = open(config_dir)?;
    let listener = match TcpListener::bind(("::", DEFAULT_PORT)).await {
        Ok(listener) => listener,
        Err(_) => TcpListener::bind(("0.0.0.0", DEFAULT_PORT))
            .await
            .with_context(|| format!("could not listen on port {DEFAULT_PORT}"))?,
    };
    tokio::spawn(Arc::clone(&relay).run(listener));
    Ok(relay)
}

/// The link members register with: the relay's pin and secret at the host
/// the owner named, else the hub's most widely reachable route.
fn join_link(relay: &Relay, config: &HubConfig, routes: &[HostRoute]) -> Option<String> {
    let host = config
        .relay_host
        .as_deref()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(|host| host.trim_matches(['[', ']']).to_owned())
        .or_else(|| relay_host(routes))?;
    Some(
        JoinLink {
            host,
            port: DEFAULT_PORT,
            fingerprint: relay.fingerprint().to_owned(),
            secret: relay.secret(),
        }
        .to_string(),
    )
}

/// The host of the route a device elsewhere is likeliest to reach: a public
/// one, then a name that is not `.local`, then an overlay, then the LAN.
pub(super) fn relay_host(routes: &[HostRoute]) -> Option<String> {
    routes
        .iter()
        .filter_map(|route| {
            let host = route_host(&route.address)?;
            let rank = match route.kind {
                HostRouteKind::Public => 0,
                HostRouteKind::Hostname if !host.ends_with(".local") => 1,
                HostRouteKind::Overlay => 2,
                HostRouteKind::Lan => 3,
                HostRouteKind::Hostname => 4,
                HostRouteKind::Relay => return None,
            };
            Some((rank, host))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, host)| host)
}

/// The host in a route address (`host:port`, `[v6]:port`, or a URL), without
/// brackets.
fn route_host(address: &str) -> Option<String> {
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    let authority = rest.split('/').next()?;
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split_once(']')?.0
    } else if authority.parse::<IpAddr>().is_ok() {
        authority
    } else {
        authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
    };
    (!host.is_empty()).then(|| host.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(address: &str, kind: HostRouteKind) -> HostRoute {
        HostRoute {
            address: address.to_owned(),
            kind,
            provider: None,
        }
    }

    #[test]
    fn a_route_address_gives_its_host() {
        assert_eq!(
            route_host("192.168.1.5:17878").as_deref(),
            Some("192.168.1.5")
        );
        assert_eq!(route_host("[fd7a::1]:17878").as_deref(), Some("fd7a::1"));
        assert_eq!(route_host("fd7a::1").as_deref(), Some("fd7a::1"));
        assert_eq!(
            route_host("wss://hub.example.com/remote").as_deref(),
            Some("hub.example.com")
        );
        assert_eq!(route_host("").as_deref(), None);
    }

    #[test]
    fn the_relay_is_named_by_the_hubs_widest_route() {
        let lan = route("192.168.1.5:17878", HostRouteKind::Lan);
        let mdns = route("hub.local:17878", HostRouteKind::Hostname);
        let mesh = route("100.64.0.7:17878", HostRouteKind::Overlay);
        let name = route("hub.example.com:17878", HostRouteKind::Hostname);
        let public = route("203.0.113.9:17878", HostRouteKind::Public);
        let relay = route("rustcode-relay://other:17880", HostRouteKind::Relay);
        assert_eq!(relay_host(&[relay.clone()]), None);
        assert_eq!(
            relay_host(&[mdns.clone(), lan.clone()]).as_deref(),
            Some("192.168.1.5")
        );
        assert_eq!(
            relay_host(&[lan.clone(), mesh.clone(), mdns.clone()]).as_deref(),
            Some("100.64.0.7")
        );
        assert_eq!(
            relay_host(&[lan.clone(), mesh.clone(), name.clone()]).as_deref(),
            Some("hub.example.com")
        );
        assert_eq!(
            relay_host(&[relay, lan, mesh, name, public]).as_deref(),
            Some("203.0.113.9")
        );
    }
}
