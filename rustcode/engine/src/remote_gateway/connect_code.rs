//! Connect codes: what the host's owner carries to another computer to pair
//! it, with no accept step on this host (`rustcode machines code`, Settings ›
//! Machines › Create connect code).
//!
//! One code has two forms, both redeemable once within ten minutes
//! ([`super::pairing::PairingState::issue_machine`]):
//!
//! - **Short**, `K7MQ-29XH-TP4W-NRD6`: sixteen characters of
//!   [`ALPHABET`], 80 bits. Four bits of version, one IPv4 address on
//!   [`super::DEFAULT_PORT`] (`0.0.0.0` when the code carries none and the
//!   address is typed next to it), and a 44-bit secret. Read aloud, typed by
//!   hand.
//! - **Long**, `rustcode://connect/<base64url JSON>`: the gateway's id, its
//!   name, every route it answers on and the 256-bit credential. Copied and
//!   pasted, over SSH too. A device checks the gateway's id before it sends
//!   the credential.
//!
//! The client crate (`rustcode-client`, `connect_code.rs`) parses both; its
//! tests decode what this module encodes.

use super::DEFAULT_PORT;
use super::address::{HostRoute, HostRouteKind};
use super::handshake::Secret;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddr};

/// The short code's characters: no `0`, `1`, `I` or `O`, which read alike.
pub const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// The version nibble of a short code this module writes.
pub const SHORT_VERSION: u8 = 1;

/// Characters in a short code, without separators.
pub const SHORT_CHARS: usize = 16;

/// The scheme and path a long code starts with.
pub const LINK_PREFIX: &str = "rustcode://connect/";

/// The version of the long form's JSON.
pub const LINK_VERSION: u32 = 1;

/// What a long code carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectLink {
    pub v: u32,
    /// The gateway the code pairs with: a device refuses to send the
    /// credential to an address that describes itself as another one.
    pub gateway_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    /// Every address the gateway answers on, best first.
    pub routes: Vec<HostRoute>,
    pub credential: Secret,
}

/// The short form, grouped by four: `K7MQ-29XH-TP4W-NRD6`. `address` is left
/// out (`0.0.0.0`) when there is none to carry.
pub fn short_code(address: Option<Ipv4Addr>, secret: u64) -> String {
    let address = u128::from(u32::from(address.unwrap_or(Ipv4Addr::UNSPECIFIED)));
    let secret = u128::from(secret) & ((1u128 << 44) - 1);
    let bits = (u128::from(SHORT_VERSION) << 76) | (address << 44) | secret;
    let mut code = String::with_capacity(SHORT_CHARS + 3);
    for index in 0..SHORT_CHARS {
        if index > 0 && index % 4 == 0 {
            code.push('-');
        }
        let shift = 5 * (SHORT_CHARS - 1 - index);
        code.push(ALPHABET[((bits >> shift) & 31) as usize] as char);
    }
    code
}

/// The long form: [`LINK_PREFIX`] and the link as base64url JSON.
pub fn long_code(link: &ConnectLink) -> String {
    let json = serde_json::to_vec(link).expect("a connect link always serializes");
    format!(
        "{LINK_PREFIX}{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
    )
}

/// The address a short code carries: the first overlay IPv4 route on the
/// default port, since a mesh address works from anywhere on the mesh, else
/// the first LAN one. `None` when the gateway listens on another port or has
/// no such address; the short code then carries none and the address is
/// typed next to it.
pub fn short_address(routes: &[HostRoute]) -> Option<Ipv4Addr> {
    let ipv4 = |kind: HostRouteKind| {
        routes.iter().find_map(|route| {
            if route.kind != kind {
                return None;
            }
            match route.address.parse::<SocketAddr>().ok()? {
                SocketAddr::V4(address) if address.port() == DEFAULT_PORT => Some(*address.ip()),
                _ => None,
            }
        })
    };
    ipv4(HostRouteKind::Overlay).or_else(|| ipv4(HostRouteKind::Lan))
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
    fn a_short_code_is_sixteen_characters_in_four_groups() {
        let code = short_code(Some(Ipv4Addr::new(100, 92, 13, 44)), 0xabc_def0_1234);
        assert_eq!(code.len(), 19);
        assert_eq!(code.matches('-').count(), 3);
        assert!(
            code.chars()
                .filter(|c| *c != '-')
                .all(|c| ALPHABET.contains(&(c as u8)))
        );
        // Deterministic: the same address and secret give the same code.
        assert_eq!(
            code,
            short_code(Some(Ipv4Addr::new(100, 92, 13, 44)), 0xabc_def0_1234)
        );
        assert_ne!(code, short_code(None, 0xabc_def0_1234));
    }

    #[test]
    fn the_short_address_prefers_an_overlay_on_the_default_port() {
        let routes = [
            route("192.168.1.20:17879", HostRouteKind::Lan),
            route("100.92.13.44:17879", HostRouteKind::Overlay),
        ];
        assert_eq!(short_address(&routes), Some(Ipv4Addr::new(100, 92, 13, 44)));
        assert_eq!(
            short_address(&routes[..1]),
            Some(Ipv4Addr::new(192, 168, 1, 20))
        );
        // Another port, IPv6 or a name: the short code carries no address.
        assert_eq!(
            short_address(&[
                route("192.168.1.20:18000", HostRouteKind::Lan),
                route("[fd00::2]:17879", HostRouteKind::Lan),
                route("studio.local:17879", HostRouteKind::Hostname),
            ]),
            None
        );
    }

    #[test]
    fn a_long_code_is_a_link_with_base64url_json() {
        let link = ConnectLink {
            v: LINK_VERSION,
            gateway_id: "gw-1".into(),
            host_name: Some("Studio".into()),
            routes: vec![route("192.168.1.20:17879", HostRouteKind::Lan)],
            credential: Secret::new("cred"),
        };
        let code = long_code(&link);
        let encoded = code.strip_prefix(LINK_PREFIX).unwrap();
        let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .unwrap();
        assert_eq!(
            String::from_utf8(json).unwrap(),
            r#"{"v":1,"gateway_id":"gw-1","host_name":"Studio","routes":[{"address":"192.168.1.20:17879","kind":"lan"}],"credential":"cred"}"#
        );
    }
}
