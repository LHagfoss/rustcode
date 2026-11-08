//! Bonjour advertises a service name, the protocol version, the gateway
//! identifier and the optional handshakes the gateway takes, never pairing
//! material. The identifier lets a paired device recognise its host at a new
//! address; it is a name, not a credential.
//!
//! One implementation on every OS: the `mdns-sd` responder runs on its own
//! thread inside the gateway, so neither `dns-sd` (macOS) nor Avahi (Linux)
//! is needed, and Windows announces itself the same way.
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

/// The service type phones browse for (`_rustcode._tcp.` in `local.`).
pub const SERVICE_TYPE: &str = "_rustcode._tcp.local.";
/// The TXT record's `protocol` value: the remote protocol major version.
pub const TXT_PROTOCOL: &str = "1";
/// The TXT record's `caps` value: comma-separated optional handshakes.
/// `connect_requests` means a device may send `connect_request` and have the
/// host's owner accept it ([`super::connect`]); `desktops` that a desktop app
/// may pair as a device ([`crate::remote::host_capability::DESKTOPS`]).
pub const TXT_CAPS: &str = "connect_requests,desktops";
/// The instance name when the machine has no usable name.
const FALLBACK_NAME: &str = "RustCode";
/// How long a clean stop waits for the goodbye to go out.
const STOP_TIMEOUT: Duration = Duration::from_secs(1);

/// The TXT record, in the order the subprocesses used to write it.
pub fn txt_record(gateway_id: &str) -> [(&'static str, String); 3] {
    [
        ("protocol", TXT_PROTOCOL.to_owned()),
        ("gateway", gateway_id.to_owned()),
        ("caps", TXT_CAPS.to_owned()),
    ]
}

/// The `.local.` host the service record points at. It is RustCode's own
/// name, derived from the gateway identifier, rather than the machine's: the
/// OS responder (mDNSResponder, Avahi, Windows' DNS client) already owns the
/// machine's name, and two responders answering for one name conflict. The
/// phone resolves it to whichever of this machine's addresses is on its
/// network.
pub fn host_label(gateway_id: &str) -> String {
    let suffix: String = gateway_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect::<String>()
        .to_ascii_lowercase();
    if suffix.is_empty() {
        "rustcode.local.".to_owned()
    } else {
        format!("rustcode-{suffix}.local.")
    }
}

/// The service as announced: `instance` is the name a phone lists.
fn service_info(
    instance: &str,
    port: u16,
    gateway_id: &str,
    interfaces: Vec<IfKind>,
) -> mdns_sd::Result<ServiceInfo> {
    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        instance,
        &host_label(gateway_id),
        (),
        port,
        &txt_record(gateway_id)[..],
    )
    // The responder fills in each interface's own addresses, and follows
    // them as DHCP or Wi-Fi changes them.
    .map(ServiceInfo::enable_addr_auto)?;
    info.set_interfaces(interfaces);
    Ok(info)
}

/// A live announcement. Dropping it withdraws the record without waiting;
/// [`Advertisement::stop`] waits for the goodbye to go out.
pub struct Advertisement {
    daemon: Option<ServiceDaemon>,
    fullname: String,
}

impl Advertisement {
    /// Announce the gateway on its listener interface, unless its advertised
    /// address is loopback-only. A failure leaves the gateway running without Bonjour.
    pub fn start(port: u16, bind: IpAddr, loopback: bool, gateway_id: &str) -> Option<Self> {
        if loopback {
            return None;
        }
        let name = super::gateway::host_name().unwrap_or_else(|| FALLBACK_NAME.into());
        let interfaces = interfaces_for_bind(bind);
        if interfaces.is_empty() {
            eprintln!("Bonjour announcement unavailable: no suitable local network interface");
            return None;
        }
        match Self::announce(&name, port, gateway_id, interfaces) {
            Ok(advertisement) => Some(advertisement),
            Err(error) => {
                eprintln!("Bonjour announcement unavailable: {error}");
                None
            }
        }
    }

    /// Test helper that announces on an explicitly selected interface set.
    fn announce(
        instance: &str,
        port: u16,
        gateway_id: &str,
        interfaces: Vec<IfKind>,
    ) -> mdns_sd::Result<Self> {
        let daemon = responder(interfaces.clone())?;
        let info = service_info(instance, port, gateway_id, interfaces)?;
        let fullname = info.get_fullname().to_owned();
        if let Err(error) = daemon.register(info) {
            let _ = daemon.shutdown();
            return Err(error);
        }
        Ok(Self {
            daemon: Some(daemon),
            fullname,
        })
    }

    /// The full service name, `<instance>._rustcode._tcp.local.`.
    pub fn fullname(&self) -> &str {
        &self.fullname
    }

    /// Withdraw the record and wait (briefly) until the responder has sent
    /// its goodbye, so phones drop the host at once instead of after the
    /// record's lifetime.
    pub async fn stop(mut self) {
        let Some(daemon) = self.daemon.take() else {
            return;
        };
        let fullname = std::mem::take(&mut self.fullname);
        let _ = tokio::task::spawn_blocking(move || {
            let deadline = Instant::now() + STOP_TIMEOUT;
            if let Ok(status) = daemon.unregister(&fullname) {
                let _ = status.recv_timeout(STOP_TIMEOUT);
            }
            if let Ok(status) = daemon.shutdown() {
                let _ = status.recv_timeout(deadline.saturating_duration_since(Instant::now()));
            }
        })
        .await;
    }
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        if let Some(daemon) = self.daemon.take() {
            // Queued in order on the responder's thread: the goodbye goes
            // out before it exits, as long as the process lives that long.
            let _ = daemon.unregister(&self.fullname);
            let _ = daemon.shutdown();
        }
    }
}

/// A responder on the network interfaces, or on loopback alone. Interface
/// rules apply in order, the last match winning.
fn responder(interfaces: Vec<IfKind>) -> mdns_sd::Result<ServiceDaemon> {
    let daemon = ServiceDaemon::new()?;
    daemon.disable_interface(IfKind::All)?;
    if !interfaces.is_empty() {
        daemon.enable_interface(interfaces)?;
    }
    Ok(daemon)
}

/// Keep local-network discovery on the selected listener interface. This
/// avoids publishing a LAN gateway through unrelated VPN, bridge or host-only
/// adapters; overlay discovery has its own path.
fn interfaces_for_bind(bind: IpAddr) -> Vec<IfKind> {
    let candidates = super::address::discovery_candidates();
    interface_kinds(&candidates, bind)
}

fn interface_kinds(candidates: &[super::address::Candidate], bind: IpAddr) -> Vec<IfKind> {
    let mut names: Vec<String> = Vec::new();
    for candidate in candidates {
        if !super::address::is_local_network_interface(&candidate.interface)
            || (!bind.is_unspecified() && candidate.address != bind)
            || names.iter().any(|name| name == &candidate.interface)
        {
            continue;
        }
        names.push(candidate.interface.clone());
    }
    names.into_iter().map(IfKind::Name).collect()
}

fn is_dialable_address(address: IpAddr) -> bool {
    !address.is_loopback()
        && !address.is_unspecified()
        && !address.is_multicast()
        && match address {
            IpAddr::V4(ip) => !ip.is_link_local() && !ip.is_broadcast(),
            IpAddr::V6(ip) => !ip.is_unicast_link_local(),
        }
}

fn instance_name(fullname: &str) -> String {
    fullname
        .strip_suffix(SERVICE_TYPE)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(fullname)
        .replace("\\.", ".")
}

/// A gateway found on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredGateway {
    /// The instance name: the machine's name as its user knows it.
    pub name: String,
    pub port: u16,
    pub addresses: Vec<IpAddr>,
    pub gateway_id: Option<String>,
    pub protocol: Option<String>,
    pub caps: Vec<String>,
}

impl DiscoveredGateway {
    fn from_resolved(service: &mdns_sd::ResolvedService) -> Self {
        let fullname = service.get_fullname();
        let name = instance_name(fullname);
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
            .filter(|address| is_dialable_address(*address))
            .collect();
        addresses.sort();
        Self {
            name,
            port: service.get_port(),
            addresses,
            gateway_id: text("gateway"),
            protocol: text("protocol"),
            caps: text("caps")
                .map(|caps| caps.split(',').map(str::to_owned).collect())
                .unwrap_or_default(),
        }
    }
}

/// Browse for RustCode gateways for `duration` and return each one resolved.
/// Blocking: run it off the async runtime.
pub fn browse(duration: Duration) -> mdns_sd::Result<Vec<DiscoveredGateway>> {
    browse_with(
        responder(interface_kinds(
            &super::address::discovery_candidates(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        ))?,
        duration,
        |_| false,
    )
}

/// Browse until `duration` passes or `enough` is satisfied by what was found.
fn browse_with(
    daemon: ServiceDaemon,
    duration: Duration,
    enough: impl Fn(&[DiscoveredGateway]) -> bool,
) -> mdns_sd::Result<Vec<DiscoveredGateway>> {
    let events = daemon.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + duration;
    let mut found: Vec<DiscoveredGateway> = Vec::new();
    while let Some(left) = deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
    {
        let Ok(event) = events.recv_timeout(left) else {
            break;
        };
        match event {
            ServiceEvent::ServiceResolved(service) => {
                let gateway = DiscoveredGateway::from_resolved(&service);
                found.retain(|known| known.name != gateway.name);
                found.push(gateway);
                if enough(&found) {
                    break;
                }
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                let removed = instance_name(&fullname);
                found.retain(|known| known.name != removed);
            }
            _ => {}
        }
    }
    let _ = daemon.stop_browse(SERVICE_TYPE);
    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_txt_record_is_what_the_phone_reads() {
        let record = txt_record("8e1f4b7a");
        assert_eq!(
            record,
            [
                ("protocol", "1".to_owned()),
                ("gateway", "8e1f4b7a".to_owned()),
                ("caps", "connect_requests,desktops".to_owned()),
            ]
        );
        let info = service_info(
            "Lars's MacBook",
            17879,
            "8e1f4b7a",
            vec![IfKind::Name("en0".to_owned())],
        )
        .unwrap();
        assert_eq!(info.get_type(), "_rustcode._tcp.local.");
        assert_eq!(info.get_fullname(), "Lars's MacBook._rustcode._tcp.local.");
        assert_eq!(info.get_port(), 17879);
        assert_eq!(info.get_property_val_str("protocol"), Some("1"));
        assert_eq!(info.get_property_val_str("gateway"), Some("8e1f4b7a"));
        assert_eq!(
            info.get_property_val_str("caps"),
            Some("connect_requests,desktops")
        );
        assert_eq!(info.get_properties().len(), 3);
        assert!(info.is_addr_auto());
        // Nothing that could pair a device is in the record.
        for property in info.get_properties().iter() {
            assert!(
                ["protocol", "gateway", "caps"].contains(&property.key()),
                "{}",
                property.key()
            );
        }
    }

    #[test]
    fn the_host_name_is_rustcodes_own_and_a_valid_label() {
        assert_eq!(
            host_label("8E1F4B7A9C0D1E2F3A4B"),
            "rustcode-8e1f4b7a9c0d.local."
        );
        assert_eq!(host_label("gw-8e1f"), "rustcode-gw8e1f.local.");
        assert_eq!(host_label("---"), "rustcode.local.");
        let info = service_info(
            "Studio",
            1,
            "8e1f4b7a",
            vec![IfKind::Name("en0".to_owned())],
        )
        .unwrap();
        assert_eq!(info.get_hostname(), "rustcode-8e1f4b7a.local.");
    }

    #[test]
    fn a_loopback_only_gateway_is_not_announced() {
        assert!(
            Advertisement::start(17879, IpAddr::V4(Ipv4Addr::LOCALHOST), true, "8e1f4b7a")
                .is_none()
        );
    }

    #[test]
    fn interface_selection_follows_the_bind_and_skips_non_lan_adapters() {
        let candidates = [
            super::super::address::Candidate {
                interface: "en0".to_owned(),
                address: "192.168.1.20".parse().unwrap(),
            },
            super::super::address::Candidate {
                interface: "Wi-Fi".to_owned(),
                address: "192.168.1.21".parse().unwrap(),
            },
            super::super::address::Candidate {
                interface: "wt0".to_owned(),
                address: "100.90.16.237".parse().unwrap(),
            },
            super::super::address::Candidate {
                interface: "vEthernet (Default Switch)".to_owned(),
                address: "172.24.0.1".parse().unwrap(),
            },
        ];
        let selected = interface_kinds(&candidates, "192.168.1.20".parse().unwrap());
        assert_eq!(
            selected
                .iter()
                .filter_map(interface_name)
                .collect::<Vec<_>>(),
            ["en0"]
        );
        let wildcard = interface_kinds(&candidates, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(
            wildcard
                .iter()
                .filter_map(interface_name)
                .collect::<Vec<_>>(),
            ["en0", "Wi-Fi"]
        );
    }

    fn interface_name(interface: &IfKind) -> Option<&str> {
        match interface {
            IfKind::Name(name) => Some(name),
            _ => None,
        }
    }

    #[test]
    fn discovered_addresses_exclude_unscoped_and_unroutable_values() {
        for address in [
            "fe80::1".parse().unwrap(),
            "169.254.1.2".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
            "224.0.0.1".parse().unwrap(),
        ] {
            assert!(!is_dialable_address(address), "{address}");
        }
        assert!(is_dialable_address("192.168.1.20".parse().unwrap()));
        assert!(is_dialable_address("2001:db8::20".parse().unwrap()));
    }

    #[test]
    fn service_removal_matches_the_decoded_instance_name_exactly() {
        assert_eq!(
            instance_name("Studio\\.Lab._rustcode._tcp.local."),
            "Studio.Lab"
        );
        assert_ne!(instance_name("Studio-2._rustcode._tcp.local."), "Studio");
    }

    #[tokio::test]
    async fn an_announcement_starts_and_stops_cleanly() {
        let interfaces = vec![IfKind::All];
        let Ok(advertisement) =
            Advertisement::announce("Lifecycle test", 17879, "lifecycle", interfaces.clone())
        else {
            return;
        };
        assert_eq!(
            advertisement.fullname(),
            "Lifecycle test._rustcode._tcp.local."
        );
        let started = Instant::now();
        advertisement.stop().await;
        assert!(started.elapsed() < STOP_TIMEOUT * 3);
        // Dropping without a stop withdraws it too, without blocking.
        if let Ok(dropped) = Advertisement::announce("Dropped test", 17880, "dropped", interfaces) {
            drop(dropped);
        }
    }

    /// Announce and browse over the loopback interface on this machine. It
    /// needs multicast on loopback, which macOS has and Linux's `lo` usually
    /// does not, so it is not part of the default run:
    /// `cargo test -p rustcode --lib discovery -- --ignored`.
    #[test]
    #[ignore = "needs multicast on the loopback interface"]
    fn a_browse_finds_the_announcement_on_loopback() {
        let gateway_id = format!("loop{}", std::process::id());
        let instance = format!("RustCode loopback {}", std::process::id());
        let loopback = vec![IfKind::LoopbackV4];
        let advertisement =
            Advertisement::announce(&instance, 17881, &gateway_id, loopback.clone())
                .expect("announce");
        let found = browse_with(
            responder(loopback).unwrap(),
            Duration::from_secs(10),
            |found| found.iter().any(|gateway| gateway.name == instance),
        )
        .unwrap();
        drop(advertisement);
        let gateway = found
            .iter()
            .find(|gateway| gateway.name == instance)
            .unwrap_or_else(|| panic!("not found among {found:?}"));
        assert_eq!(gateway.port, 17881);
        assert_eq!(gateway.gateway_id.as_deref(), Some(gateway_id.as_str()));
        assert_eq!(gateway.protocol.as_deref(), Some("1"));
        assert_eq!(gateway.caps, ["connect_requests", "desktops"]);
        assert!(
            !gateway.addresses.is_empty() && gateway.addresses.iter().all(IpAddr::is_loopback),
            "{gateway:?}"
        );
    }
}
