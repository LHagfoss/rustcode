//! Bind and advertised addresses for the gateway listener.
//!
//! The bind address is where the socket listens; the advertised address is
//! what pairing details tell a device to dial. They differ whenever the bind
//! is a wildcard, so an unspecified address is never advertised and an
//! ambiguous choice is never guessed.

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
}

/// Resolve `--bind` and `--advertise`.
///
/// A loopback or specific bind advertises itself unless told otherwise. A
/// wildcard bind needs `--advertise` unless exactly one candidate exists;
/// the error lists the candidates. `candidates` is passed in so the decision
/// is testable; production callers use [`candidate_addresses`].
pub fn plan_listen(
    bind: &str,
    advertise: Option<&str>,
    candidates: &[Candidate],
) -> Result<ListenPlan, AddressError> {
    let bind: IpAddr = bind
        .trim()
        .parse()
        .map_err(|_| AddressError::InvalidBind(bind.to_string()))?;
    if let Some(advertise) = advertise {
        return Ok(ListenPlan {
            bind,
            advertise: AdvertisedHost::parse(advertise)?,
        });
    }
    if !bind.is_unspecified() {
        return Ok(ListenPlan {
            bind,
            advertise: AdvertisedHost::from_ip(bind)?,
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
        }),
        _ => Err(AddressError::AdvertiseRequired {
            candidates: reachable,
        }),
    }
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

#[cfg(not(unix))]
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

    #[cfg(unix)]
    #[test]
    fn candidates_exclude_loopback_and_unspecified_addresses() {
        for candidate in candidate_addresses() {
            assert!(!candidate.address.is_loopback(), "{candidate:?}");
            assert!(!candidate.address.is_unspecified(), "{candidate:?}");
            assert!(!candidate.interface.is_empty());
        }
    }
}
