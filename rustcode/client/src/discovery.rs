//! Bonjour discovery of machines that accept desktop clients.
//!
//! The gateway announces `_rustcode._tcp` with a TXT record naming its
//! protocol version, its stable `gateway` identifier and its optional
//! handshakes in `caps`. A machine that lists `desktops` there takes a
//! `connect_request` from a desktop app or the command line, which is what
//! `rustcode machines discover` and `rustcode machines add --discover` look
//! for. The record never carries anything that could pair a device.

use crate::registry::RouteKind;
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// The service type the gateway announces (`_rustcode._tcp.` in `local.`),
/// the same one the phone browses for.
pub const SERVICE_TYPE: &str = "_rustcode._tcp.local.";

/// How long a browse listens by default.
pub const BROWSE_DURATION: Duration = Duration::from_secs(4);

/// A machine found on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMachine {
    /// The instance name: the computer's name as its user knows it.
    pub name: String,
    pub port: u16,
    /// The machine's dialable addresses, sorted.
    pub addresses: Vec<IpAddr>,
    /// The gateway identifier, which is also the `host_id` a pairing reports.
    pub gateway_id: Option<String>,
    pub protocol: Option<String>,
    /// The optional handshakes the machine's record lists.
    pub caps: Vec<String>,
}

impl DiscoveredMachine {
    /// Whether this machine takes an ask-to-connect request from a desktop
    /// client (`caps=desktops`).
    pub fn accepts_desktops(&self) -> bool {
        self.caps.iter().any(|cap| cap == "desktops")
    }

    /// The address to dial for this machine: its first address on the kind of
    /// network this computer shares with it, with the announcement's port.
    ///
    /// Addresses come back sorted, which puts IPv4 first, and a LAN address
    /// before a public one; that is enough while route racing is step 1.8.
    pub fn address(&self) -> Option<String> {
        let mut addresses = self.addresses.clone();
        addresses.sort_by_key(|address| {
            (
                route_rank(crate::registry::classify_address(&address.to_string())),
                !address.is_ipv4(),
            )
        });
        addresses.first().map(|address| match address {
            IpAddr::V4(ip) => format!("{ip}:{}", self.port),
            IpAddr::V6(ip) => format!("[{ip}]:{}", self.port),
        })
    }

    /// Every address as a `host:port` route, in preference order.
    pub fn routes(&self) -> Vec<crate::registry::Route> {
        let mut routes: Vec<crate::registry::Route> = self
            .addresses
            .iter()
            .map(|address| {
                let address = match address {
                    IpAddr::V4(ip) => format!("{ip}:{}", self.port),
                    IpAddr::V6(ip) => format!("[{ip}]:{}", self.port),
                };
                crate::registry::Route::new(address)
            })
            .collect();
        routes.sort_by_key(|route| route_rank(route.kind));
        routes
    }
}

fn route_rank(kind: RouteKind) -> u8 {
    match kind {
        RouteKind::Lan => 0,
        RouteKind::Tailscale => 1,
        RouteKind::Vpn => 2,
        RouteKind::Public => 3,
    }
}

/// Browse for `duration` and return each machine that takes desktop clients,
/// by name. Blocking: run it off the async runtime.
pub fn browse(duration: Duration) -> mdns_sd::Result<Vec<DiscoveredMachine>> {
    let daemon = ServiceDaemon::new()?;
    let events = match daemon.browse(SERVICE_TYPE) {
        Ok(events) => events,
        Err(error) => {
            let _ = daemon.shutdown();
            return Err(error);
        }
    };
    let deadline = Instant::now() + duration;
    let mut found: Vec<DiscoveredMachine> = Vec::new();
    while let Some(left) = deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
    {
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        match event {
            ServiceEvent::ServiceResolved(service) => {
                if let Some(machine) = from_resolved(&service) {
                    found.retain(|known| known.name != machine.name);
                    found.push(machine);
                }
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                let removed = instance_name(&fullname);
                found.retain(|known| known.name != removed);
            }
            ServiceEvent::ServiceFound(_, _) => {}
            _ => {}
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    found.retain(DiscoveredMachine::accepts_desktops);
    found.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(found)
}

/// Browse without blocking the caller's thread.
pub async fn browse_async(duration: Duration) -> mdns_sd::Result<Vec<DiscoveredMachine>> {
    tokio::task::spawn_blocking(move || browse(duration))
        .await
        .map_err(|error| mdns_sd::Error::Msg(format!("discovery task failed: {error}")))?
}

fn from_resolved(service: &mdns_sd::ResolvedService) -> Option<DiscoveredMachine> {
    let text = |key: &str| {
        service
            .get_property_val_str(key)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let mut addresses: Vec<IpAddr> = service
        .get_addresses()
        .iter()
        .map(|address| address.to_ip_addr())
        .filter(|address| is_dialable(*address))
        .collect();
    addresses.sort();
    Some(DiscoveredMachine {
        name: instance_name(service.get_fullname()),
        port: service.get_port(),
        addresses,
        gateway_id: text("gateway"),
        protocol: text("protocol"),
        caps: text("caps")
            .map(|caps| caps.split(',').map(str::to_owned).collect())
            .unwrap_or_default(),
    })
}

/// A record without an address cannot be dialled, and an unroutable one is
/// never worth announcing.
fn is_dialable(address: IpAddr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && match address {
            IpAddr::V4(ip) => !ip.is_link_local() && !ip.is_broadcast(),
            IpAddr::V6(ip) => !ip.is_unicast_link_local(),
        }
}

/// `Studio._rustcode._tcp.local.` is the machine called `Studio`.
fn instance_name(fullname: &str) -> String {
    fullname
        .strip_suffix(SERVICE_TYPE)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(fullname)
        .replace("\\.", ".")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine(name: &str, caps: &[&str], addresses: &[&str]) -> DiscoveredMachine {
        DiscoveredMachine {
            name: name.to_owned(),
            port: 17879,
            addresses: addresses.iter().map(|a| a.parse().unwrap()).collect(),
            gateway_id: Some("gw-8e1f4b7a".into()),
            protocol: Some("1".into()),
            caps: caps.iter().map(|cap| (*cap).to_owned()).collect(),
        }
    }

    #[test]
    fn only_machines_that_take_desktop_clients_are_listed() {
        assert!(machine("Studio", &["connect_requests", "desktops"], &[]).accepts_desktops());
        assert!(!machine("Old", &["connect_requests"], &[]).accepts_desktops());
        assert!(!machine("Bare", &[], &[]).accepts_desktops());
    }

    #[test]
    fn the_dialled_address_prefers_a_lan_route() {
        let found = machine(
            "Studio",
            &["desktops"],
            &["100.92.13.44", "192.168.1.20", "203.0.113.9"],
        );
        assert_eq!(found.address().as_deref(), Some("192.168.1.20:17879"));
        let routes = found.routes();
        assert_eq!(
            routes.iter().map(|route| route.kind).collect::<Vec<_>>(),
            [RouteKind::Lan, RouteKind::Vpn, RouteKind::Public]
        );
        assert_eq!(routes[0].address, "192.168.1.20:17879");
        // IPv6 keeps its brackets so the port cannot be read as a segment.
        let v6 = machine("Six", &["desktops"], &["fd00::1"]);
        assert_eq!(v6.address().as_deref(), Some("[fd00::1]:17879"));
        assert_eq!(machine("Nowhere", &["desktops"], &[]).address(), None);
    }

    #[test]
    fn an_instance_name_is_the_computer_name() {
        assert_eq!(instance_name("Studio._rustcode._tcp.local."), "Studio");
        assert_eq!(
            instance_name("Studio\\.Lab._rustcode._tcp.local."),
            "Studio.Lab"
        );
    }

    #[test]
    fn unroutable_addresses_are_not_offered() {
        for address in [
            "fe80::1",
            "169.254.1.2",
            "127.0.0.1",
            "0.0.0.0",
            "224.0.0.1",
        ] {
            assert!(!is_dialable(address.parse().unwrap()), "{address}");
        }
        assert!(is_dialable("192.168.1.20".parse().unwrap()));
        assert!(is_dialable("2001:db8::20".parse().unwrap()));
    }
}
