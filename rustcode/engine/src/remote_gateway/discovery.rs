//! Bonjour advertises a service name, the protocol version, the gateway
//! identifier and the optional handshakes the gateway takes, never pairing
//! material. The identifier lets a paired device recognise its host at a new
//! address; it is a name, not a credential.
//!
//! One implementation on every OS: the `mdns-sd` responder runs on its own
//! thread inside the gateway, so neither `dns-sd` (macOS) nor Avahi (Linux)
//! is needed, and Windows announces itself the same way.
use mdns_sd::{DaemonEvent, IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant, SystemTime};

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
/// How often the announcement re-checks what it covers. A network change is
/// picked up within this interval.
pub(super) const RECHECK_INTERVAL: Duration = Duration::from_secs(15);
/// A re-announcement this long after the last one keeps a record alive that
/// went missing without anyone reporting why: a flushed cache, a dropped
/// goodbye from a previous instance, a responder that stopped answering.
const REANNOUNCE_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// A wall clock that moved this much further than the monotonic clock means
/// the machine slept in between.
const SLEEP_GAP: Duration = Duration::from_secs(30);

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
    instance: String,
    port: u16,
    gateway_id: String,
    interfaces: Vec<IfKind>,
    events: Option<mdns_sd::Receiver<DaemonEvent>>,
}

impl Advertisement {
    /// Announce the gateway on its listener interface, unless its advertised
    /// address is loopback-only. A failure leaves the gateway running without Bonjour.
    pub fn start(port: u16, bind: IpAddr, loopback: bool, gateway_id: &str) -> Option<Self> {
        Self::try_start(port, bind, loopback, gateway_id, true)
    }

    /// Like [`Advertisement::start`], for the watchdog's periodic retry. A
    /// machine with no local network interface yet would otherwise write the
    /// same explanation into the gateway's log on every re-check.
    pub fn retry(port: u16, bind: IpAddr, loopback: bool, gateway_id: &str) -> Option<Self> {
        Self::try_start(port, bind, loopback, gateway_id, false)
    }

    fn try_start(
        port: u16,
        bind: IpAddr,
        loopback: bool,
        gateway_id: &str,
        report: bool,
    ) -> Option<Self> {
        if loopback {
            return None;
        }
        let name = super::gateway::host_name().unwrap_or_else(|| FALLBACK_NAME.into());
        let interfaces = interfaces_for(bind);
        if interfaces.is_empty() {
            if report {
                eprintln!("Bonjour announcement unavailable: no suitable local network interface");
            }
            return None;
        }
        match Self::announce(&name, port, gateway_id, interfaces) {
            Ok(advertisement) => Some(advertisement),
            Err(error) => {
                if report {
                    eprintln!("Bonjour announcement unavailable: {error}");
                }
                None
            }
        }
    }

    /// Announces on an explicitly selected interface set.
    fn announce(
        instance: &str,
        port: u16,
        gateway_id: &str,
        interfaces: Vec<IfKind>,
    ) -> mdns_sd::Result<Self> {
        let daemon = responder(interfaces.clone())?;
        let events = daemon.monitor().ok();
        let info = service_info(instance, port, gateway_id, interfaces.clone())?;
        let fullname = info.get_fullname().to_owned();
        if let Err(error) = daemon.register(info) {
            let _ = daemon.shutdown();
            return Err(error);
        }
        Ok(Self {
            daemon: Some(daemon),
            fullname,
            instance: instance.to_owned(),
            port,
            gateway_id: gateway_id.to_owned(),
            interfaces,
            events,
        })
    }

    /// Whether the responder is still running and has not reported an error.
    /// Drains its events: a name conflict that renamed the service instance is
    /// adopted, so [`Advertisement::stop`] withdraws the record that exists.
    pub fn healthy(&mut self) -> bool {
        if self.daemon.is_none() {
            return false;
        }
        let Some(events) = self.events.as_ref() else {
            return true;
        };
        if events.is_disconnected() {
            return false;
        }
        let mut healthy = true;
        while let Ok(event) = events.try_recv() {
            match event {
                DaemonEvent::Error(_) => healthy = false,
                DaemonEvent::NameChange(change) => {
                    if change.original.eq_ignore_ascii_case(&self.fullname) {
                        self.fullname = change.new_name;
                    }
                }
                _ => {}
            }
        }
        healthy
    }

    /// Point the responder at `interfaces` and announce again. Registering the
    /// same instance replaces the record and sends an unsolicited response, so
    /// a record lost to sleep, a Wi-Fi change or a stale interface list comes
    /// back without a new responder. `false` means a fresh [`Advertisement`]
    /// is the way back.
    pub fn refresh(&mut self, interfaces: Vec<IfKind>) -> bool {
        let Some(daemon) = self.daemon.as_ref() else {
            return false;
        };
        if !same_interfaces(&self.interfaces, &interfaces) {
            if daemon.disable_interface(IfKind::All).is_err()
                || (!interfaces.is_empty() && daemon.enable_interface(interfaces.clone()).is_err())
            {
                return false;
            }
            self.interfaces = interfaces.clone();
        }
        let Ok(info) = service_info(&self.instance, self.port, &self.gateway_id, interfaces) else {
            return false;
        };
        self.fullname = info.get_fullname().to_owned();
        daemon.register(info).is_ok()
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

/// Whether two interface selections pick the same interfaces. [`IfKind`] is
/// not comparable, so names are compared by value and every other kind by
/// discriminant.
fn same_interfaces(left: &[IfKind], right: &[IfKind]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| match (left, right) {
                (IfKind::Name(left), IfKind::Name(right)) => left == right,
                _ => std::mem::discriminant(left) == std::mem::discriminant(right),
            })
}

/// Keep local-network discovery on the selected listener interface. This
/// avoids publishing a LAN gateway through unrelated VPN, bridge or host-only
/// adapters; overlay discovery has its own path.
pub(super) fn interfaces_for(bind: IpAddr) -> Vec<IfKind> {
    interface_kinds(&super::address::discovery_candidates(), bind)
}

/// Candidates an announcement on `bind` covers: local network interfaces
/// only, and the advertised address when the listener is bound to one.
fn covered<'a>(
    candidates: &'a [super::address::Candidate],
    bind: IpAddr,
) -> impl Iterator<Item = &'a super::address::Candidate> {
    candidates.iter().filter(move |candidate| {
        super::address::is_local_network_interface(&candidate.interface)
            && (bind.is_unspecified() || candidate.address == bind)
    })
}

fn interface_kinds(candidates: &[super::address::Candidate], bind: IpAddr) -> Vec<IfKind> {
    let mut names: Vec<&str> = Vec::new();
    for candidate in covered(candidates, bind) {
        if !names.contains(&candidate.interface.as_str()) {
            names.push(&candidate.interface);
        }
    }
    names
        .into_iter()
        .map(|name| IfKind::Name(name.to_owned()))
        .collect()
}

/// What an announcement covers: the interface name and address of every
/// candidate a device could reach it on. Sorted, so the same set in another
/// order compares equal.
pub(super) type Coverage = Vec<(String, IpAddr)>;

fn coverage(candidates: &[super::address::Candidate], bind: IpAddr) -> Coverage {
    let mut coverage: Coverage = covered(candidates, bind)
        .map(|candidate| (candidate.interface.clone(), candidate.address))
        .collect();
    coverage.sort();
    coverage.dedup();
    coverage
}

/// Whether an announcement built for `previous` no longer covers what the
/// machine offers now: an interface went up or down, or its address changed.
pub(super) fn coverage_changed(previous: &Coverage, current: &Coverage) -> bool {
    previous != current
}

/// How long the machine slept, if it did. `Instant` stops while the machine is
/// asleep and the wall clock does not, so a wall clock that moved further than
/// the monotonic one means sleep. A clock set backwards reports nothing.
pub(super) fn slept(
    monotonic_elapsed: Duration,
    wall_elapsed: Option<Duration>,
) -> Option<Duration> {
    let gap = wall_elapsed?.checked_sub(monotonic_elapsed)?;
    (gap > SLEEP_GAP).then_some(gap)
}

/// Why the announcement should be refreshed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refresh {
    /// Nothing worth acting on.
    Nothing,
    /// The interface or address set an announcement covers changed.
    Interfaces,
    /// The machine woke from sleep, so its record may be gone.
    Woke,
    /// The heartbeat interval passed since the last announcement.
    Heartbeat,
}

/// Watches for the two things that silently invalidate an announcement, the
/// interfaces it covers having changed and the machine having slept, plus a
/// slow heartbeat. Polling is cheap and bounded; a change is acted on within
/// one [`RECHECK_INTERVAL`].
pub(super) struct Watchdog {
    coverage: Coverage,
    last_check: Instant,
    last_wall: SystemTime,
    last_announce: Instant,
}

impl Watchdog {
    pub(super) fn new(bind: IpAddr) -> Self {
        let now = Instant::now();
        Self {
            coverage: coverage(&super::address::discovery_candidates(), bind),
            last_check: now,
            last_wall: SystemTime::now(),
            last_announce: now,
        }
    }

    /// Re-check what `bind` covers now and whether the machine slept. A
    /// returned reason other than [`Refresh::Nothing`] means the caller
    /// announces again, which is why this also moves the heartbeat on.
    pub(super) fn poll(&mut self, bind: IpAddr) -> Refresh {
        let now = Instant::now();
        let wall = SystemTime::now();
        let elapsed = now.saturating_duration_since(self.last_check);
        let wall_elapsed = wall.duration_since(self.last_wall).ok();
        self.last_check = now;
        self.last_wall = wall;
        let current = coverage(&super::address::discovery_candidates(), bind);
        let changed = coverage_changed(&self.coverage, &current);
        self.coverage = current;
        let refresh = if changed {
            Refresh::Interfaces
        } else if slept(elapsed, wall_elapsed).is_some() {
            Refresh::Woke
        } else if now.saturating_duration_since(self.last_announce) >= REANNOUNCE_INTERVAL {
            Refresh::Heartbeat
        } else {
            Refresh::Nothing
        };
        if refresh != Refresh::Nothing {
            self.last_announce = now;
        }
        refresh
    }
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

    fn candidate(interface: &str, address: &str) -> super::super::address::Candidate {
        super::super::address::Candidate {
            interface: interface.to_owned(),
            address: address.parse().unwrap(),
        }
    }

    #[test]
    fn an_announcement_is_rebuilt_when_what_it_covers_changes() {
        let wildcard = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let before = coverage(
            &[
                candidate("en0", "192.168.1.20"),
                candidate("wt0", "100.90.16.237"),
            ],
            wildcard,
        );
        assert_eq!(
            before,
            [("en0".to_owned(), "192.168.1.20".parse().unwrap())]
        );

        // The same interfaces, announced in another order, are not a change.
        let reordered = coverage(
            &[
                candidate("wt0", "100.90.16.237"),
                candidate("en0", "192.168.1.20"),
            ],
            wildcard,
        );
        assert!(!coverage_changed(&before, &reordered));
        assert!(!coverage_changed(&before, &before.clone()));

        // An interface that came up, one that went away, and a new lease on a
        // covered interface all need the record rebuilt.
        let joined = coverage(
            &[
                candidate("en0", "192.168.1.20"),
                candidate("en1", "10.0.0.5"),
            ],
            wildcard,
        );
        assert!(coverage_changed(&before, &joined));
        assert!(coverage_changed(&joined, &before));
        let moved = coverage(&[candidate("en0", "192.168.1.99")], wildcard);
        assert!(coverage_changed(&before, &moved));
        assert!(coverage_changed(&before, &coverage(&[], wildcard)));

        // A listener bound to one address covers only that address, so a new
        // address on another interface alone does not rebuild its record.
        let bound = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        let only_en0 = coverage(&[candidate("en0", "192.168.1.20")], bound);
        assert_eq!(
            coverage(
                &[
                    candidate("en0", "192.168.1.20"),
                    candidate("en1", "10.0.0.5"),
                ],
                bound,
            ),
            only_en0
        );
    }

    #[test]
    fn the_coverage_leaves_overlay_and_bridge_adapters_out() {
        let wildcard = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let candidates = [
            candidate("en0", "192.168.1.20"),
            candidate("utun4", "100.90.16.237"),
            candidate("bridge100", "192.168.64.1"),
            candidate("vEthernet (Default Switch)", "172.24.0.1"),
        ];
        assert_eq!(
            coverage(&candidates, wildcard),
            [("en0".to_owned(), "192.168.1.20".parse().unwrap())]
        );
    }

    #[test]
    fn a_clock_jump_means_the_machine_slept() {
        let secs = Duration::from_secs;
        // Both clocks moved together: no sleep.
        assert_eq!(slept(secs(15), Some(secs(15))), None);
        // Drift between the clocks is not sleep either.
        assert_eq!(slept(secs(15), Some(secs(15) + SLEEP_GAP)), None);
        // The wall clock moved an hour further than the monotonic clock.
        assert_eq!(slept(secs(15), Some(secs(3615))), Some(secs(3600)));
        // A clock set back, or no wall clock at all, reports nothing.
        assert_eq!(slept(secs(15), Some(secs(5))), None);
        assert_eq!(slept(secs(15), None), None);
    }

    #[test]
    fn interface_selections_compare_by_name() {
        let en0 = vec![IfKind::Name("en0".to_owned())];
        assert!(same_interfaces(&en0, &en0.clone()));
        assert!(!same_interfaces(&en0, &[IfKind::Name("en1".to_owned())]));
        assert!(!same_interfaces(&en0, &[]));
        assert!(same_interfaces(
            &[IfKind::All, IfKind::LoopbackV4],
            &[IfKind::All, IfKind::LoopbackV4]
        ));
        assert!(!same_interfaces(
            &[IfKind::All, IfKind::LoopbackV4],
            &[IfKind::All, IfKind::IPv4]
        ));
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
        let Ok(mut advertisement) =
            Advertisement::announce("Lifecycle test", 17879, "lifecycle", interfaces.clone())
        else {
            return;
        };
        assert_eq!(
            advertisement.fullname(),
            "Lifecycle test._rustcode._tcp.local."
        );
        // The watchdog's re-announcement: still healthy, and then accepts the
        // same and a new interface set without a new responder.
        assert!(advertisement.healthy());
        assert!(advertisement.refresh(interfaces.clone()));
        assert!(advertisement.healthy());
        assert!(advertisement.refresh(vec![IfKind::Name("lo0".to_owned())]));
        assert_eq!(
            advertisement.fullname(),
            "Lifecycle test._rustcode._tcp.local.",
            "re-announcing keeps the instance name"
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
