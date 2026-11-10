//! Bind and advertised addresses for the gateway listener.
//!
//! The bind address is where the socket listens; the advertised address is
//! what pairing details tell a device to dial. They differ whenever the bind
//! is a wildcard, so an unspecified address is never advertised and an
//! ambiguous choice is never guessed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};

/// A local interface address a device could plausibly dial.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub interface: String,
    pub address: IpAddr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    InvalidBind(String),
    InvalidAdvertise(String),
    /// `0.0.0.0`, `::` and other addresses no device can dial.
    UnroutableAdvertise(String),
    /// A wildcard bind with zero or several candidates and no `--advertise`.
    AdvertiseRequired {
        candidates: Vec<Candidate>,
    },
}

impl fmt::Display for AddressError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBind(bind) => write!(formatter, "invalid bind address: {bind}"),
            Self::InvalidAdvertise(host) => write!(
                formatter,
                "invalid advertised address: {host} (expected an IP address or host name, without a port)"
            ),
            Self::UnroutableAdvertise(host) => write!(
                formatter,
                "refusing to advertise {host}: devices cannot dial it; pass --advertise with a reachable address"
            ),
            Self::AdvertiseRequired { candidates } => {
                write!(
                    formatter,
                    "the bind address accepts connections on every interface, so the address to advertise is ambiguous; pass --advertise <address>"
                )?;
                if candidates.is_empty() {
                    write!(formatter, " (no candidate interface address was found)")?;
                } else {
                    write!(formatter, "\ncandidate addresses:")?;
                    for candidate in candidates {
                        write!(
                            formatter,
                            "\n  {}  ({})",
                            candidate.address, candidate.interface
                        )?;
                    }
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AddressError {}

/// A host a device can dial. Construction refuses unspecified, broadcast and
/// multicast addresses, so no later code path can put one in pairing details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedHost(HostKind);

#[derive(Debug, Clone, PartialEq, Eq)]
enum HostKind {
    Ip(IpAddr),
    Name(String),
}

impl AdvertisedHost {
    pub fn parse(host: &str) -> Result<Self, AddressError> {
        let host = host.trim();
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Self::from_ip(ip);
        }
        // A name must look like one: this also keeps `0`, `0.0` and other
        // numeric spellings of the unspecified address out.
        let valid_name = !host.is_empty()
            && host.len() <= 253
            && host.chars().any(|c| c.is_ascii_alphabetic())
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            });
        if !valid_name {
            return Err(AddressError::InvalidAdvertise(host.to_string()));
        }
        Ok(Self(HostKind::Name(host.to_ascii_lowercase())))
    }

    pub fn from_ip(ip: IpAddr) -> Result<Self, AddressError> {
        let ip = ip.to_canonical();
        let unroutable = ip.is_unspecified()
            || ip.is_multicast()
            || matches!(ip, IpAddr::V4(v4) if v4.is_broadcast());
        if unroutable {
            return Err(AddressError::UnroutableAdvertise(ip.to_string()));
        }
        Ok(Self(HostKind::Ip(ip)))
    }

    pub fn is_loopback(&self) -> bool {
        match &self.0 {
            HostKind::Ip(ip) => ip.is_loopback(),
            HostKind::Name(name) => name == "localhost",
        }
    }

    /// The IP address, when the host is one rather than a name.
    pub fn ip(&self) -> Option<IpAddr> {
        match &self.0 {
            HostKind::Ip(ip) => Some(*ip),
            HostKind::Name(_) => None,
        }
    }
}

/// `host:port` as shown in pairing details and embedded in the QR payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvertisedAddress {
    host: AdvertisedHost,
    port: u16,
}

impl AdvertisedAddress {
    pub fn new(host: &str, port: u16) -> Result<Self, AddressError> {
        Ok(AdvertisedHost::parse(host)?.with_port(port))
    }

    pub fn host(&self) -> &AdvertisedHost {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl AdvertisedHost {
    pub fn with_port(self, port: u16) -> AdvertisedAddress {
        AdvertisedAddress { host: self, port }
    }
}

impl fmt::Display for AdvertisedAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host.0 {
            HostKind::Ip(IpAddr::V6(ip)) => write!(formatter, "[{ip}]:{}", self.port),
            HostKind::Ip(IpAddr::V4(ip)) => write!(formatter, "{ip}:{}", self.port),
            HostKind::Name(name) => write!(formatter, "{name}:{}", self.port),
        }
    }
}

/// Where to listen and what to advertise, before the port is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenPlan {
    pub bind: IpAddr,
    pub advertise: AdvertisedHost,
    /// The bind was `auto`: it follows the machine's addresses.
    pub automatic_bind: bool,
    /// The advertised host was given (`--advertise`, `[remote] advertise`)
    /// rather than derived from the bind; it never moves on its own.
    pub fixed_advertise: bool,
}

/// Resolve `--bind` and `--advertise`.
///
/// `auto` chooses a local LAN address, falling back to loopback offline. A
/// loopback or specific bind advertises itself unless told otherwise. A
/// wildcard bind needs `--advertise` unless exactly one candidate exists;
/// the error lists the candidates. `candidates` is passed in so the decision
/// is testable; production callers use [`candidate_addresses`].
pub fn plan_listen(
    bind: &str,
    advertise: Option<&str>,
    candidates: &[Candidate],
) -> Result<ListenPlan, AddressError> {
    plan_listen_with_preference(bind, advertise, candidates, None)
}

/// Like [`plan_listen`], but an `auto` bind first tries interfaces matching
/// `prefer_interface` (e.g. `"netbird"` from `[remote] prefer_interface` or
/// `serve --prefer-interface`). A mesh address is stable across Wi-Fi changes
/// and reachable off-LAN, which is what a phone usually wants; when no
/// interface matches, selection falls back to the LAN-first default.
pub fn plan_listen_with_preference(
    bind: &str,
    advertise: Option<&str>,
    candidates: &[Candidate],
    prefer_interface: Option<&str>,
) -> Result<ListenPlan, AddressError> {
    let automatic_bind = bind.trim() == "auto";
    let bind: IpAddr = if automatic_bind {
        automatic_address_with_preference(candidates, prefer_interface)
    } else {
        bind.trim()
            .parse()
            .map_err(|_| AddressError::InvalidBind(bind.to_string()))?
    };
    if let Some(advertise) = advertise {
        return Ok(ListenPlan {
            bind,
            advertise: AdvertisedHost::parse(advertise)?,
            automatic_bind,
            fixed_advertise: true,
        });
    }
    if !bind.is_unspecified() {
        return Ok(ListenPlan {
            bind,
            advertise: AdvertisedHost::from_ip(bind)?,
            automatic_bind,
            fixed_advertise: false,
        });
    }
    // An IPv4 wildcard cannot accept IPv6 peers; `::` is usually dual-stack.
    let reachable: Vec<Candidate> = candidates
        .iter()
        .filter(|candidate| bind.is_ipv6() || candidate.address.is_ipv4())
        .cloned()
        .collect();
    match reachable.as_slice() {
        [only] => Ok(ListenPlan {
            bind,
            advertise: AdvertisedHost::from_ip(only.address)?,
            automatic_bind,
            fixed_advertise: false,
        }),
        _ => Err(AddressError::AdvertiseRequired {
            candidates: reachable,
        }),
    }
}

/// True for interface names belonging to a NetBird mesh: `wt0`-style
/// WireGuard devices on Linux/Windows, or anything with netbird in the name.
/// (On macOS NetBird uses a generic `utun` device shared with other VPNs, so
/// there `--advertise <host>.netbird.cloud` is the reliable route.)
pub fn is_netbird_interface(interface: &str) -> bool {
    let name = interface.to_ascii_lowercase();
    name.starts_with("wt") || name.contains("netbird")
}

/// True when `interface` is selected by a `prefer_interface` value: a
/// case-insensitive substring match, plus the `"netbird"` alias for NetBird's
/// `wt`-prefixed devices, whose names never mention netbird.
pub fn interface_matches_preference(interface: &str, prefer: &str) -> bool {
    if prefer.is_empty() {
        return false;
    }
    let name = interface.to_ascii_lowercase();
    let want = prefer.to_ascii_lowercase();
    name.contains(&want) || (want == "netbird" && is_netbird_interface(interface))
}

/// Prefer Wi-Fi/Ethernet over VPN and virtual networks, so the QR works on
/// the phone's ordinary Wi-Fi. Bind only the selected interface, not all of
/// them. Interface names cover macOS and the usual Linux naming schemes.
fn automatic_address(candidates: &[Candidate]) -> IpAddr {
    // UDP connect asks the OS which local address it would route from. No
    // packet is sent and internet access is not required for this lookup.
    let route = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .ok()
        .and_then(|socket| {
            socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
            socket.local_addr().ok().map(|address| address.ip())
        });
    automatic_address_with_route(candidates, route)
}

fn automatic_address_with_preference(
    candidates: &[Candidate],
    prefer_interface: Option<&str>,
) -> IpAddr {
    if let Some(prefer) = prefer_interface
        && !prefer.trim().is_empty()
    {
        let preferred: Vec<&Candidate> = eligible_candidates(candidates)
            .into_iter()
            .filter(|candidate| interface_matches_preference(&candidate.interface, prefer.trim()))
            .collect();
        if let Some(address) = preferred
            .iter()
            .min_by_key(|candidate| {
                (
                    !matches!(candidate.address, IpAddr::V4(ip) if ip.is_private()),
                    candidate.address,
                )
            })
            .map(|candidate| candidate.address)
        {
            return address;
        }
    }
    automatic_address(candidates)
}

/// Candidates a device could plausibly dial: up, non-loopback, routable, and
/// not a container/VM bridge.
fn eligible_candidates(candidates: &[Candidate]) -> Vec<&Candidate> {
    candidates
        .iter()
        .filter(|candidate| {
            let interface = candidate.interface.as_str();
            let address = candidate.address;
            !address.is_loopback()
                && !address.is_unspecified()
                && !address.is_multicast()
                && !matches!(address, IpAddr::V4(ip) if ip.is_link_local() || ip.is_broadcast())
                && !matches!(address, IpAddr::V6(ip) if ip.is_unicast_link_local())
                && is_auto_candidate_interface(interface)
        })
        .collect()
}

fn is_auto_candidate_interface(interface: &str) -> bool {
    let name = interface.to_ascii_lowercase();
    ![
        "awdl",
        "br",
        "bridge",
        "docker",
        "hyper-v",
        "llw",
        "p2p",
        "vbox",
        "vethernet",
        "veth",
        "virbr",
        "virtual",
        "vmnet",
        "vmware",
        "vnic",
        "wsl",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

/// Exclude adapters that are normally tunnels, peer-to-peer links, or
/// virtual/host-only networks from Bonjour. Windows adapter type is checked
/// separately because `Candidate` intentionally keeps the interface name
/// only, for `--prefer-interface` and the address planner.
pub(super) fn is_local_network_interface(interface: &str) -> bool {
    let name = interface.to_ascii_lowercase();
    ![
        "awdl",
        "bluetooth",
        "br",
        "bridge",
        "docker",
        "hyper-v",
        "llw",
        "p2p",
        "tap",
        "tailscale",
        "tun",
        "utun",
        "vbox",
        "vethernet",
        "veth",
        "virbr",
        "virtual",
        "vmnet",
        "vmware",
        "vnic",
        "wg",
        "wireguard",
        "wt",
        "wsl",
        "zt",
        "zerotier",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn is_lan_interface(interface: &str) -> bool {
    let name = interface.to_ascii_lowercase();
    [
        "en",
        "eth",
        "wl",
        "wi-fi",
        "ethernet",
        "wireless network connection",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}

fn automatic_address_with_route(candidates: &[Candidate], route: Option<IpAddr>) -> IpAddr {
    eligible_candidates(candidates)
        .into_iter()
        .min_by_key(|candidate| {
            let physical = is_lan_interface(&candidate.interface);
            let rank = match (physical, candidate.address) {
                (true, IpAddr::V4(ip)) if ip.is_private() => 0,
                (true, IpAddr::V4(_)) => 1,
                (true, IpAddr::V6(_)) => 2,
                (false, IpAddr::V4(_)) => 3,
                (false, IpAddr::V6(_)) => 4,
            };
            (rank, Some(candidate.address) != route, candidate.address)
        })
        .map(|candidate| candidate.address)
        .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
}

/// Most fallback addresses a pairing QR code carries, so it stays scannable.
pub const MAX_ALTERNATES: usize = 4;

/// Other `host:port` addresses a wildcard listener (`0.0.0.0` or `::`) is
/// reachable on, for a device that cannot dial the advertised one: private
/// IPv4 on Wi-Fi/Ethernet first, then mesh VPN addresses (100.64.0.0/10,
/// NetBird and Tailscale), then global IPv6, at most [`MAX_ALTERNATES`].
/// The advertised address, loopback, link-local and container/VM bridges are
/// left out, and so is everything when the listener is bound to one address:
/// nothing else reaches it then.
pub fn alternate_addresses(
    bind: IpAddr,
    advertised: &AdvertisedAddress,
    candidates: &[Candidate],
) -> Vec<String> {
    if !bind.is_unspecified() {
        return Vec::new();
    }
    let advertised_ip = advertised.host().ip();
    let mut ranked: Vec<(u8, IpAddr)> = eligible_candidates(candidates)
        .into_iter()
        .filter(|candidate| bind.is_ipv6() || candidate.address.is_ipv4())
        .map(|candidate| {
            (
                candidate.interface.as_str(),
                candidate.address.to_canonical(),
            )
        })
        .filter(|(_, address)| Some(*address) != advertised_ip)
        .filter_map(|(interface, address)| {
            let rank = match address {
                IpAddr::V4(ip) if ip.is_private() && is_lan_interface(interface) => 0,
                IpAddr::V4(ip) if is_shared_address_space(ip) => 1,
                IpAddr::V6(ip) if is_global_ipv6(ip) => 2,
                _ => return None,
            };
            Some((rank, address))
        })
        .collect();
    ranked.sort();
    ranked.dedup_by_key(|(_, address)| *address);
    ranked
        .into_iter()
        .filter_map(|(_, address)| AdvertisedHost::from_ip(address).ok())
        .take(MAX_ALTERNATES)
        .map(|host| host.with_port(advertised.port()).to_string())
        .collect()
}

/// How a device reaches this host over one route. A gateway reports each
/// address it answers on with the kind of network it is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostRouteKind {
    /// A private IPv4 address on a local network interface (Wi-Fi, Ethernet).
    Lan,
    /// A `100.64.0.0/10` mesh address, as NetBird and Tailscale hand out.
    Vpn,
    /// A host name: the machine's `.local` name, or another advertised name.
    Hostname,
    /// A public address or name. Plain LAN traffic is unencrypted, so a
    /// client should prefer the other kinds.
    Public,
}

/// One address a device can dial this host at, as the handshake reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostRoute {
    /// `host:port` a device dials.
    pub address: String,
    pub kind: HostRouteKind,
}

/// Most routes one handshake reports, so the frame stays small.
pub const MAX_ROUTES: usize = 8;

/// Which kind a local address is, when a device could plausibly dial it.
/// Global IPv4 is left out: a NAT usually stops it, and the host reports the
/// names it is reachable by instead. Unique-local IPv6 counts as a VPN.
fn route_kind(address: IpAddr) -> Option<HostRouteKind> {
    match address {
        IpAddr::V4(ip) if is_shared_address_space(ip) => Some(HostRouteKind::Vpn),
        IpAddr::V4(ip) if ip.is_private() => Some(HostRouteKind::Lan),
        IpAddr::V4(_) => None,
        IpAddr::V6(ip) if (ip.segments()[0] & 0xfe00) == 0xfc00 => Some(HostRouteKind::Vpn),
        IpAddr::V6(_) => None,
    }
}

/// The addresses a device can reach this gateway at, best first: the
/// advertised one, then every LAN and mesh address the machine has, then its
/// `.local` name. The pairing QR code carries the same set as `alternates`;
/// this is the live list a connected device learns and adds to its own routes.
/// Empty for a loopback-only listener: no other machine can reach it.
pub fn host_routes(
    advertised: &AdvertisedAddress,
    candidates: &[Candidate],
    local_name: Option<&str>,
) -> Vec<HostRoute> {
    if advertised.host().is_loopback() {
        return Vec::new();
    }
    let port = advertised.port();
    let mut routes = Vec::new();
    let advertised_kind = match advertised.host() {
        host if host.ip().is_none() => HostRouteKind::Hostname,
        host => host
            .ip()
            .and_then(route_kind)
            .unwrap_or(HostRouteKind::Public),
    };
    routes.push(HostRoute {
        address: advertised.to_string(),
        kind: advertised_kind,
    });
    let mut ranked: Vec<(u8, HostRoute)> = eligible_candidates(candidates)
        .into_iter()
        .filter_map(|candidate| {
            let kind = route_kind(candidate.address.to_canonical())?;
            let rank = match kind {
                HostRouteKind::Lan => 0,
                HostRouteKind::Vpn => 1,
                HostRouteKind::Hostname => 2,
                HostRouteKind::Public => 3,
            };
            Some((
                rank,
                HostRoute {
                    address: format!(
                        "{}",
                        AdvertisedHost::from_ip(candidate.address)
                            .ok()?
                            .with_port(port)
                    ),
                    kind,
                },
            ))
        })
        .collect();
    ranked.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then(left.1.address.cmp(&right.1.address))
    });
    for (_, route) in ranked {
        if routes.len() >= MAX_ROUTES {
            break;
        }
        if routes.iter().all(|known| known.address != route.address) {
            routes.push(route);
        }
    }
    if let Some(name) = local_name.map(str::trim).filter(|name| !name.is_empty()) {
        let address = if name.contains(':') {
            format!("[{name}]:{port}")
        } else {
            format!("{name}:{port}")
        };
        if routes.len() < MAX_ROUTES && routes.iter().all(|known| known.address != address) {
            routes.push(HostRoute {
                address,
                kind: HostRouteKind::Hostname,
            });
        }
    }
    routes
}

/// 100.64.0.0/10, the shared address space mesh VPNs (NetBird, Tailscale)
/// hand out.
fn is_shared_address_space(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    first == 100 && (64..128).contains(&second)
}

/// Global unicast IPv6 (2000::/3): not unique-local, link-local or special.
fn is_global_ipv6(ip: std::net::Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xe000) == 0x2000
}

/// Non-loopback addresses of interfaces that are up. Link-local addresses are
/// left out: IPv6 ones need a scope a phone cannot know, IPv4 ones mean DHCP
/// failed.
#[cfg(unix)]
pub fn candidate_addresses() -> Vec<Candidate> {
    use std::net::Ipv6Addr;

    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs writes a list head we free below on success.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let mut cursor = list;
    while !cursor.is_null() {
        // SAFETY: cursor is a live node of the list getifaddrs returned.
        let entry = unsafe { &*cursor };
        cursor = entry.ifa_next;
        let up = entry.ifa_flags & libc::IFF_UP as libc::c_uint != 0;
        let loopback = entry.ifa_flags & libc::IFF_LOOPBACK as libc::c_uint != 0;
        if entry.ifa_addr.is_null() || !up || loopback {
            continue;
        }
        // SAFETY: ifa_addr is non-null and begins with a sockaddr header.
        let family = i32::from(unsafe { (*entry.ifa_addr).sa_family });
        let address = if family == libc::AF_INET {
            // SAFETY: AF_INET addresses are sockaddr_in.
            let socket = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
            IpAddr::V4(Ipv4Addr::from(u32::from_be(socket.sin_addr.s_addr)))
        } else if family == libc::AF_INET6 {
            // SAFETY: AF_INET6 addresses are sockaddr_in6.
            let socket = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in6>() };
            IpAddr::V6(Ipv6Addr::from(socket.sin6_addr.s6_addr))
        } else {
            continue;
        };
        let link_local = match address {
            IpAddr::V4(v4) => v4.is_link_local(),
            IpAddr::V6(v6) => v6.is_unicast_link_local(),
        };
        if link_local || address.is_loopback() || address.is_unspecified() {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated string owned by the list.
        let interface = unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) }
            .to_string_lossy()
            .into_owned();
        candidates.push(Candidate { interface, address });
    }
    // SAFETY: list came from getifaddrs and is not used after this point.
    unsafe { libc::freeifaddrs(list) };
    candidates
}

/// Windows: the adapters' friendly names ("Wi-Fi", "Ethernet"), so
/// `--prefer-interface` takes what the user sees in Settings.
#[cfg(windows)]
pub fn candidate_addresses() -> Vec<Candidate> {
    windows_interfaces()
        .into_iter()
        .filter_map(windows_candidate)
        .collect()
}

#[cfg(windows)]
pub(super) fn discovery_candidates() -> Vec<Candidate> {
    windows_interfaces()
        .into_iter()
        .filter(|interface| !interface.is_p2p() && is_local_network_interface(&interface.name))
        .filter_map(windows_candidate)
        .collect()
}

#[cfg(windows)]
fn windows_interfaces() -> Vec<if_addrs::Interface> {
    if_addrs::get_if_addrs().unwrap_or_default()
}

#[cfg(windows)]
fn windows_candidate(interface: if_addrs::Interface) -> Option<Candidate> {
    if !interface.is_oper_up() || interface.is_loopback() {
        return None;
    }
    let address = interface.ip();
    let link_local = match address {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unicast_link_local(),
    };
    (!link_local && !address.is_loopback() && !address.is_unspecified()).then(|| Candidate {
        interface: interface.name,
        address,
    })
}

#[cfg(unix)]
pub(super) fn discovery_candidates() -> Vec<Candidate> {
    candidate_addresses()
        .into_iter()
        .filter(|candidate| is_local_network_interface(&candidate.interface))
        .collect()
}

#[cfg(not(any(unix, windows)))]
pub(super) fn discovery_candidates() -> Vec<Candidate> {
    Vec::new()
}

#[cfg(not(any(unix, windows)))]
pub fn candidate_addresses() -> Vec<Candidate> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(interface: &str, address: &str) -> Candidate {
        Candidate {
            interface: interface.into(),
            address: address.parse().unwrap(),
        }
    }

    #[test]
    fn automatic_bind_uses_lan_instead_of_loopback_or_vpn() {
        let candidates = [
            candidate("wt0", "100.90.16.237"),
            candidate("bridge100", "192.168.64.1"),
            candidate("en0", "192.168.31.185"),
        ];
        let plan = plan_listen("auto", None, &candidates).unwrap();
        assert_eq!(plan.bind, "192.168.31.185".parse::<IpAddr>().unwrap());
        assert_eq!(
            plan.advertise.with_port(17879).to_string(),
            "192.168.31.185:17879"
        );
        let plan = plan_listen("auto", Some("mac.example"), &candidates).unwrap();
        assert_eq!(plan.bind, "192.168.31.185".parse::<IpAddr>().unwrap());
        assert_eq!(plan.advertise.with_port(7).to_string(), "mac.example:7");
    }

    #[test]
    fn prefer_interface_selects_netbird_before_lan() {
        let candidates = [
            candidate("en0", "192.168.31.185"),
            candidate("wt0", "100.90.16.237"),
        ];
        // Default stays LAN-first: a full-tunnel VPN must not hide the LAN
        // address from a nearby phone.
        let plan = plan_listen("auto", None, &candidates).unwrap();
        assert_eq!(plan.bind, "192.168.31.185".parse::<IpAddr>().unwrap());
        // Opt-in: the mesh address is stable across Wi-Fi changes and
        // reachable off-LAN.
        for prefer in ["netbird", "wt0", "WT"] {
            let plan =
                plan_listen_with_preference("auto", None, &candidates, Some(prefer)).unwrap();
            assert_eq!(plan.bind, "100.90.16.237".parse::<IpAddr>().unwrap());
        }
        // No match falls back to the LAN default.
        let plan =
            plan_listen_with_preference("auto", None, &candidates, Some("tailscale0")).unwrap();
        assert_eq!(plan.bind, "192.168.31.185".parse::<IpAddr>().unwrap());
        // An explicit bind or advertise always wins over the preference.
        let plan =
            plan_listen_with_preference("192.168.31.185", None, &candidates, Some("netbird"))
                .unwrap();
        assert_eq!(plan.bind, "192.168.31.185".parse::<IpAddr>().unwrap());
        let plan =
            plan_listen_with_preference("auto", Some("mac.example"), &candidates, Some("netbird"))
                .unwrap();
        assert_eq!(plan.bind, "100.90.16.237".parse::<IpAddr>().unwrap());
        assert_eq!(plan.advertise.with_port(7).to_string(), "mac.example:7");
    }

    #[test]
    fn netbird_interface_alias_covers_wireguard_device_names() {
        assert!(is_netbird_interface("wt0"));
        assert!(is_netbird_interface("WT1"));
        assert!(is_netbird_interface("netbird0"));
        assert!(!is_netbird_interface("en0"));
        assert!(!is_netbird_interface("utun4"));
        assert!(!is_netbird_interface("tailscale0"));
        assert!(interface_matches_preference("wt0", "netbird"));
        assert!(interface_matches_preference("en0", "en"));
        assert!(!interface_matches_preference("en0", ""));
        assert!(!interface_matches_preference("en0", "netbird"));
    }

    #[test]
    fn automatic_plan_follows_the_addresses_the_machine_has_now() {
        // Resolved again with the same arguments, the plan tracks a changed
        // lease, and leaves loopback once a network is there.
        let plan = |candidates: &[Candidate]| plan_listen("auto", None, candidates).unwrap();
        let before = plan(&[candidate("en0", "192.168.31.237")]);
        let after = plan(&[candidate("en0", "192.168.31.185")]);
        assert_ne!(before, after);
        assert_eq!(after.bind, "192.168.31.185".parse::<IpAddr>().unwrap());
        assert_eq!(
            after.advertise.clone().with_port(17879).to_string(),
            "192.168.31.185:17879"
        );
        assert!(plan(&[]).bind.is_loopback());
        assert_eq!(plan(&[candidate("en0", "192.168.31.185")]), after);
        // A name the user chose stays, wherever the listener moves.
        let named = plan_listen("auto", Some("mac.example"), &[candidate("en0", "10.0.0.9")]);
        assert_eq!(
            named.unwrap().advertise.with_port(1).to_string(),
            "mac.example:1"
        );
    }

    #[test]
    fn automatic_bind_without_network_keeps_local_access() {
        let plan = plan_listen("auto", None, &[]).unwrap();
        assert!(plan.bind.is_loopback());
    }

    #[test]
    fn automatic_bind_prefers_the_active_route_among_lan_interfaces() {
        let candidates = [
            candidate("en0", "10.0.0.20"),
            candidate("en1", "192.168.1.20"),
            candidate("wt0", "100.90.16.237"),
        ];
        let lan = "192.168.1.20".parse::<IpAddr>().unwrap();
        assert_eq!(automatic_address_with_route(&candidates, Some(lan)), lan);
        // A full-tunnel VPN must not hide the LAN address from a nearby phone.
        let vpn = "100.90.16.237".parse::<IpAddr>().unwrap();
        assert_eq!(
            automatic_address_with_route(&candidates, Some(vpn)),
            "10.0.0.20".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            automatic_address_with_route(&[candidate("wt0", "100.90.16.237")], Some(vpn)),
            vpn
        );
    }

    #[test]
    fn automatic_bind_ignores_virtual_and_unreachable_interfaces() {
        let candidates = [
            candidate("bridge100", "192.168.64.1"),
            candidate("br0", "172.18.0.1"),
            candidate("vmnet8", "192.168.80.1"),
            candidate("vnic0", "10.211.55.1"),
            candidate("vboxnet0", "192.168.56.1"),
            candidate("en0", "169.254.1.2"),
            candidate("en0", "fe80::1234"),
            candidate("lo0", "127.0.0.1"),
        ];
        assert!(
            plan_listen("auto", None, &candidates)
                .unwrap()
                .bind
                .is_loopback()
        );
        let plan = plan_listen("auto", None, &[candidate("eth0", "2001:db8::20")]).unwrap();
        assert_eq!(plan.advertise.with_port(5).to_string(), "[2001:db8::20]:5");
    }

    #[test]
    fn windows_friendly_lan_names_beat_tunnel_and_virtual_adapters() {
        let candidates = [
            candidate("Wi-Fi", "192.168.1.20"),
            candidate("Ethernet", "10.0.0.20"),
            candidate("WireGuard Tunnel", "100.90.16.237"),
            candidate("vEthernet (Default Switch)", "172.24.0.1"),
        ];
        let route = "100.90.16.237".parse().unwrap();
        assert_eq!(
            automatic_address_with_route(&candidates, Some(route)),
            "10.0.0.20".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn loopback_and_specific_binds_advertise_themselves() {
        let plan = plan_listen("127.0.0.1", None, &[]).unwrap();
        assert!(plan.bind.is_loopback());
        assert_eq!(
            plan.advertise.with_port(17879).to_string(),
            "127.0.0.1:17879"
        );

        let plan = plan_listen("192.168.1.20", None, &[candidate("en0", "10.0.0.1")]).unwrap();
        assert_eq!(plan.advertise.with_port(1).to_string(), "192.168.1.20:1");

        let plan = plan_listen("::1", None, &[]).unwrap();
        assert_eq!(plan.advertise.with_port(9).to_string(), "[::1]:9");
    }

    #[test]
    fn unspecified_addresses_are_never_advertised() {
        for host in [
            "0.0.0.0",
            "::",
            "::ffff:0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
        ] {
            assert!(
                matches!(
                    plan_listen("0.0.0.0", Some(host), &[]),
                    Err(AddressError::UnroutableAdvertise(_))
                ),
                "{host}"
            );
            assert!(AdvertisedAddress::new(host, 1).is_err(), "{host}");
        }
        // Numeric spellings that are not names and not valid IP literals.
        for host in [
            "0",
            "0.0",
            "",
            " ",
            "host:17879",
            "a b",
            "-a.example",
            "a..b",
        ] {
            assert!(
                matches!(
                    AdvertisedAddress::new(host, 1),
                    Err(AddressError::InvalidAdvertise(_))
                ),
                "{host}"
            );
        }
    }

    #[test]
    fn wildcard_bind_requires_an_explicit_address_when_ambiguous() {
        let candidates = [
            candidate("en0", "192.168.1.20"),
            candidate("wt0", "100.64.0.7"),
        ];
        let error = plan_listen("0.0.0.0", None, &candidates).unwrap_err();
        assert_eq!(
            error,
            AddressError::AdvertiseRequired {
                candidates: candidates.to_vec()
            }
        );
        let message = error.to_string();
        assert!(message.contains("--advertise"), "{message}");
        assert!(message.contains("192.168.1.20  (en0)"), "{message}");
        assert!(message.contains("100.64.0.7  (wt0)"), "{message}");
        assert!(!message.contains("0.0.0.0"), "{message}");

        let plan = plan_listen("0.0.0.0", Some("100.64.0.7"), &candidates).unwrap();
        assert!(plan.bind.is_unspecified());
        assert_eq!(plan.advertise.with_port(5).to_string(), "100.64.0.7:5");
    }

    #[test]
    fn wildcard_bind_uses_the_only_candidate_and_fails_with_none() {
        let plan = plan_listen("0.0.0.0", None, &[candidate("en0", "192.168.1.20")]).unwrap();
        assert_eq!(plan.advertise.with_port(5).to_string(), "192.168.1.20:5");

        assert_eq!(
            plan_listen("0.0.0.0", None, &[]),
            Err(AddressError::AdvertiseRequired { candidates: vec![] })
        );
        // An IPv6-only candidate is unreachable through an IPv4 wildcard.
        assert!(matches!(
            plan_listen("0.0.0.0", None, &[candidate("en0", "2001:db8::1")]),
            Err(AddressError::AdvertiseRequired { .. })
        ));
        let plan = plan_listen("::", None, &[candidate("en0", "2001:db8::1")]).unwrap();
        assert_eq!(plan.advertise.with_port(5).to_string(), "[2001:db8::1]:5");
    }

    #[test]
    fn host_names_are_accepted_and_normalized() {
        let address = AdvertisedAddress::new("Workstation.NetBird.cloud", 17879).unwrap();
        assert_eq!(address.to_string(), "workstation.netbird.cloud:17879");
        assert!(!address.host().is_loopback());
        assert!(
            AdvertisedAddress::new("localhost", 1)
                .unwrap()
                .host()
                .is_loopback()
        );
    }

    #[test]
    fn invalid_bind_is_rejected() {
        assert_eq!(
            plan_listen("localhost", None, &[]),
            Err(AddressError::InvalidBind("localhost".into()))
        );
    }

    #[test]
    fn candidates_exclude_loopback_and_unspecified_addresses() {
        for candidate in candidate_addresses() {
            assert!(!candidate.address.is_loopback(), "{candidate:?}");
            assert!(!candidate.address.is_unspecified(), "{candidate:?}");
            assert!(!candidate.interface.is_empty());
        }
    }

    fn machine() -> Vec<Candidate> {
        vec![
            candidate("utun4", "100.90.16.237"),
            candidate("en0", "2a01:79c:cebf:d688::1"),
            candidate("en0", "fe80::1"),
            candidate("bridge100", "192.168.64.1"),
            candidate("docker0", "172.17.0.1"),
            candidate("vmnet8", "172.16.12.1"),
            candidate("utun2", "10.8.0.2"),
            candidate("en0", "192.168.31.185"),
            candidate("en0", "fd00::1"),
            candidate("lo0", "127.0.0.1"),
            candidate("en5", "169.254.10.2"),
        ]
    }

    #[test]
    fn alternates_of_a_wildcard_bind_put_the_lan_first_and_skip_the_advertised_address() {
        let advertised = AdvertisedAddress::new("100.90.16.237", 17879).unwrap();
        let unspecified = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        assert_eq!(
            alternate_addresses(unspecified, &advertised, &machine()),
            ["192.168.31.185:17879"],
        );
        let dual_stack: IpAddr = "::".parse().unwrap();
        assert_eq!(
            alternate_addresses(dual_stack, &advertised, &machine()),
            ["192.168.31.185:17879", "[2a01:79c:cebf:d688::1]:17879"],
        );

        let lan = AdvertisedAddress::new("192.168.31.185", 17879).unwrap();
        assert_eq!(
            alternate_addresses(dual_stack, &lan, &machine()),
            ["100.90.16.237:17879", "[2a01:79c:cebf:d688::1]:17879"],
        );
    }

    #[test]
    fn alternates_of_an_advertised_name_include_every_reachable_address() {
        let named = AdvertisedAddress::new("mac.netbird.cloud", 17879).unwrap();
        assert_eq!(
            alternate_addresses(IpAddr::V4(Ipv4Addr::UNSPECIFIED), &named, &machine()),
            ["192.168.31.185:17879", "100.90.16.237:17879"],
        );
    }

    #[test]
    fn alternates_are_capped_and_deduplicated() {
        let advertised = AdvertisedAddress::new("mac.example", 1).unwrap();
        let candidates = [
            candidate("wt0", "100.64.0.9"),
            candidate("en0", "10.0.0.5"),
            candidate("en1", "10.0.0.5"),
            candidate("eth0", "172.20.0.4"),
            candidate("wlan0", "192.168.0.7"),
            candidate("tailscale0", "100.127.255.1"),
            candidate("en0", "2001:db8::5"),
        ];
        let alternates = alternate_addresses("::".parse().unwrap(), &advertised, &candidates);
        assert_eq!(
            alternates,
            [
                "10.0.0.5:1",
                "172.20.0.4:1",
                "192.168.0.7:1",
                "100.64.0.9:1"
            ],
        );
    }

    #[test]
    fn a_specific_bind_has_no_alternates() {
        let advertised = AdvertisedAddress::new("100.90.16.237", 17879).unwrap();
        let bind: IpAddr = "100.90.16.237".parse().unwrap();
        assert!(alternate_addresses(bind, &advertised, &machine()).is_empty());
    }

    #[test]
    fn host_routes_put_the_lan_first_then_the_mesh_and_the_local_name() {
        let advertised = AdvertisedAddress::new("192.168.31.185", 17879).unwrap();
        let routes = host_routes(&advertised, &machine(), Some("rustcode-8e1f4b7a.local"));
        assert_eq!(
            routes
                .iter()
                .map(|route| (route.address.as_str(), route.kind))
                .collect::<Vec<_>>(),
            [
                ("192.168.31.185:17879", HostRouteKind::Lan),
                ("10.8.0.2:17879", HostRouteKind::Lan),
                ("100.90.16.237:17879", HostRouteKind::Vpn),
                ("[fd00::1]:17879", HostRouteKind::Vpn),
                ("rustcode-8e1f4b7a.local:17879", HostRouteKind::Hostname),
            ],
        );
    }

    #[test]
    fn host_routes_name_an_advertised_name_and_skip_a_loopback_listener() {
        let named = AdvertisedAddress::new("mac.netbird.cloud", 17879).unwrap();
        let routes = host_routes(&named, &[], None);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].kind, HostRouteKind::Hostname);
        assert_eq!(routes[0].address, "mac.netbird.cloud:17879");

        let loopback = AdvertisedAddress::new("127.0.0.1", 17879).unwrap();
        assert!(host_routes(&loopback, &machine(), Some("studio.local")).is_empty());
    }

    #[test]
    fn mesh_range_is_100_64_slash_10() {
        assert!(is_shared_address_space(Ipv4Addr::new(100, 64, 0, 0)));
        assert!(is_shared_address_space(Ipv4Addr::new(100, 127, 255, 255)));
        assert!(!is_shared_address_space(Ipv4Addr::new(100, 128, 0, 1)));
        assert!(!is_shared_address_space(Ipv4Addr::new(100, 63, 0, 1)));
    }
}
