//! Connect codes as a client reads them: what another computer's owner
//! created there (`rustcode machines code`, Settings › Machines › Create
//! connect code) and carried here.
//!
//! Two forms of one code, as the gateway writes them
//! (`rustcode/engine/src/remote_gateway/connect_code.rs`):
//!
//! - **Short**, `K7MQ-29XH-TP4W-NRD6`: version, an IPv4 address on the
//!   default port (or none, and the address is typed next to it) and a
//!   44-bit secret.
//! - **Long**, `rustcode://connect/<base64url JSON>`: the gateway's id and
//!   name, every route it answers on and a 256-bit credential.
//!
//! Parsing is pure and forgiving where a person types: case, spaces, dashes
//! and the `rustcode://connect/` prefix do not matter; a character that is
//! not in the alphabet is named in the error, since `0`, `1`, `I` and `O`
//! are the ones a person misreads.

use crate::protocol::{HostRoute, Secret};
use anyhow::{Result, bail, ensure};
use base64::Engine as _;
use serde::Deserialize;
use std::net::Ipv4Addr;

/// The short code's characters, as the gateway writes them.
pub const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Characters in a short code, without separators.
pub const SHORT_CHARS: usize = 16;

/// The short code's version this client reads.
const SHORT_VERSION: u8 = 1;

/// Random bits in the short secret, and its canonical text width in hex.
const SHORT_SECRET_BITS: u32 = 44;

/// What a long code starts with.
pub const LINK_PREFIX: &str = "rustcode://connect/";

/// The long form's JSON version this client reads.
const LINK_VERSION: u32 = 1;

/// Longest input taken as a code: a long form with eight routes fits easily.
pub const MAX_CODE_CHARS: usize = 4096;

/// A connect code, parsed.
#[derive(Clone, PartialEq, Eq)]
pub enum ConnectCode {
    Short {
        /// The address the code carries, on the default port. `None` when
        /// the address has to be given next to it.
        address: Option<Ipv4Addr>,
        /// What `pair` presents: the secret as the gateway digests it.
        secret: Secret,
    },
    Long {
        gateway_id: String,
        host_name: Option<String>,
        routes: Vec<HostRoute>,
        credential: Secret,
    },
}

impl std::fmt::Debug for ConnectCode {
    /// Never the secret.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Short { address, .. } => formatter
                .debug_struct("Short")
                .field("address", address)
                .finish_non_exhaustive(),
            Self::Long {
                gateway_id, routes, ..
            } => formatter
                .debug_struct("Long")
                .field("gateway_id", gateway_id)
                .field("routes", routes)
                .finish_non_exhaustive(),
        }
    }
}

impl ConnectCode {
    /// What `pair` with `method: "machine"` presents.
    pub fn secret(&self) -> &Secret {
        match self {
            Self::Short { secret, .. } => secret,
            Self::Long { credential, .. } => credential,
        }
    }

    /// The gateway the code was made on, when the code says: only the long
    /// form does.
    pub fn gateway_id(&self) -> Option<&str> {
        match self {
            Self::Short { .. } => None,
            Self::Long { gateway_id, .. } => Some(gateway_id),
        }
    }

    /// The host name the code carries, for "Connecting to ‹name›".
    pub fn host_name(&self) -> Option<&str> {
        match self {
            Self::Short { .. } => None,
            Self::Long { host_name, .. } => host_name.as_deref(),
        }
    }

    /// The addresses to try, best first: every route of a long code, the one
    /// address a short code carries, or nothing (the address is given next
    /// to the code).
    pub fn routes(&self) -> Vec<HostRoute> {
        match self {
            Self::Short { address, .. } => address
                .map(|ip| HostRoute {
                    address: format!("{ip}:{}", crate::transport::DEFAULT_PORT),
                    kind: "lan".to_owned(),
                    provider: None,
                })
                .into_iter()
                .collect(),
            Self::Long { routes, .. } => routes.clone(),
        }
    }
}

#[derive(Deserialize)]
struct Link {
    v: u32,
    gateway_id: String,
    #[serde(default)]
    host_name: Option<String>,
    #[serde(default)]
    routes: Vec<HostRoute>,
    credential: Secret,
}

/// Read a connect code in either form.
pub fn parse(input: &str) -> Result<ConnectCode> {
    let input = input.trim();
    ensure!(!input.is_empty(), "paste or type the connect code");
    ensure!(
        input.len() <= MAX_CODE_CHARS,
        "that is too long to be a connect code"
    );
    if let Some(encoded) = strip_link_prefix(input) {
        return parse_long(encoded);
    }
    // A long code pasted without its prefix is base64url, far longer than
    // the short form's nineteen characters.
    let compact: String = input
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '\t'))
        .collect();
    if compact.len() > SHORT_CHARS + 4 {
        return parse_long(input);
    }
    parse_short(&compact)
}

fn strip_link_prefix(input: &str) -> Option<&str> {
    let lower = input.get(..LINK_PREFIX.len())?.to_ascii_lowercase();
    (lower == LINK_PREFIX).then(|| &input[LINK_PREFIX.len()..])
}

fn parse_long(encoded: &str) -> Result<ConnectCode> {
    let encoded = encoded.trim().trim_end_matches('=');
    let Ok(json) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(encoded) else {
        bail!("that is not a connect code: copy the whole link again");
    };
    let Ok(link) = serde_json::from_slice::<Link>(&json) else {
        bail!("that is not a connect code: copy the whole link again");
    };
    ensure!(
        link.v == LINK_VERSION,
        "that connect code is from a newer RustCode; update RustCode on this computer"
    );
    ensure!(
        !link.gateway_id.is_empty() && !link.credential.expose().is_empty(),
        "that connect code is incomplete: copy the whole link again"
    );
    Ok(ConnectCode::Long {
        gateway_id: link.gateway_id,
        host_name: link.host_name,
        routes: link.routes,
        credential: link.credential,
    })
}

fn parse_short(compact: &str) -> Result<ConnectCode> {
    let mut bits: u128 = 0;
    let mut count = 0usize;
    for character in compact.chars() {
        let upper = character.to_ascii_uppercase();
        let Some(value) = ALPHABET.iter().position(|&known| known as char == upper) else {
            let hint = match upper {
                '0' | 'O' | '1' | 'I' => " (a connect code has no 0, O, 1 or I)",
                _ => "",
            };
            bail!("“{character}” is not in a connect code{hint}");
        };
        bits = (bits << 5) | value as u128;
        count += 1;
    }
    ensure!(
        count == SHORT_CHARS,
        "a connect code has {SHORT_CHARS} characters, like K7MQ-29XH-TP4W-NRD6; got {count}"
    );
    let version = (bits >> 76) as u8;
    ensure!(
        version == SHORT_VERSION,
        "that connect code is from a newer RustCode, or mistyped; check it, or update RustCode on this computer"
    );
    let address = Ipv4Addr::from(((bits >> 44) & 0xffff_ffff) as u32);
    let secret = (bits & ((1u128 << SHORT_SECRET_BITS) - 1)) as u64;
    let width = SHORT_SECRET_BITS.div_ceil(4) as usize;
    Ok(ConnectCode::Short {
        address: (!address.is_unspecified()).then_some(address),
        secret: Secret::new(format!("{secret:0width$x}")),
    })
}

/// Whether `input` looks like a connect code rather than an address, so
/// `rustcode machines add` can take either.
pub fn looks_like_code(input: &str) -> bool {
    let input = input.trim();
    if strip_link_prefix(input).is_some() {
        return true;
    }
    // An address has dots, colons or brackets; a short code has none of them.
    !input.contains(['.', ':', '[', '/']) && parse(input).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustcode::remote_gateway::address::{HostRoute as GatewayRoute, HostRouteKind};
    use rustcode::remote_gateway::connect_code as gateway;

    /// What the gateway writes, this client reads: every form and every
    /// address the short code can carry.
    #[test]
    fn the_gateways_codes_parse_here() {
        for (address, secret) in [
            (Some(Ipv4Addr::new(100, 92, 13, 44)), 0xabc_def0_1234u64),
            (Some(Ipv4Addr::new(192, 168, 1, 20)), 0),
            (Some(Ipv4Addr::new(255, 255, 255, 255)), (1 << 44) - 1),
            (None, 0x123_4567_89ab),
        ] {
            let written = gateway::short_code(address, secret);
            let ConnectCode::Short {
                address: read,
                secret: read_secret,
            } = parse(&written).unwrap()
            else {
                panic!("a short code is short");
            };
            assert_eq!(read, address, "{written}");
            assert_eq!(
                read_secret.expose(),
                rustcode::remote_gateway::pairing::short_secret_text(secret)
            );
        }
        let link = gateway::ConnectLink {
            v: gateway::LINK_VERSION,
            gateway_id: "gw-0123456789abcdef".into(),
            host_name: Some("Studio".into()),
            routes: vec![GatewayRoute {
                address: "100.92.13.44:17879".into(),
                kind: HostRouteKind::Overlay,
                provider: None,
            }],
            credential: rustcode::remote_gateway::handshake::Secret::new("credential-value"),
        };
        let written = gateway::long_code(&link);
        let parsed = parse(&written).unwrap();
        assert_eq!(parsed.gateway_id(), Some("gw-0123456789abcdef"));
        assert_eq!(parsed.host_name(), Some("Studio"));
        assert_eq!(parsed.secret().expose(), "credential-value");
        assert_eq!(parsed.routes()[0].address, "100.92.13.44:17879");
        assert_eq!(parsed.routes()[0].kind, "overlay");
        // Without its prefix, and with stray whitespace around it.
        let bare = written.strip_prefix(LINK_PREFIX).unwrap();
        assert_eq!(parse(&format!("  {bare}\n")).unwrap(), parsed);
        assert!(!format!("{parsed:?}").contains("credential-value"));
    }

    #[test]
    fn a_typed_code_is_read_the_way_a_person_types_it() {
        let written = gateway::short_code(Some(Ipv4Addr::new(10, 0, 0, 2)), 77);
        let expected = parse(&written).unwrap();
        for typed in [
            written.to_lowercase(),
            written.replace('-', ""),
            written.replace('-', " "),
            format!("  {written}  "),
        ] {
            assert_eq!(parse(&typed).unwrap(), expected, "{typed}");
        }
        assert!(looks_like_code(&written));
        assert!(!looks_like_code("192.168.1.20"));
        assert!(!looks_like_code("studio.local:17879"));
        assert!(looks_like_code("rustcode://connect/xyz"));
    }

    #[test]
    fn a_mistyped_code_says_what_is_wrong() {
        let written = gateway::short_code(Some(Ipv4Addr::new(10, 0, 0, 2)), 77);
        let misread = format!("0{}", &written[1..]);
        let error = format!("{:#}", parse(&misread).unwrap_err());
        assert!(error.contains("no 0, O, 1 or I"), "{error}");
        let short = &written[..written.len() - 2];
        let error = format!("{:#}", parse(short).unwrap_err());
        assert!(error.contains("16 characters"), "{error}");
        let error = format!("{:#}", parse("").unwrap_err());
        assert!(error.contains("paste or type"), "{error}");
        let error = format!("{:#}", parse("rustcode://connect/not-json").unwrap_err());
        assert!(error.contains("copy the whole link"), "{error}");
        // A version nibble this client does not know.
        let newer = format!("A{}", &written[1..]);
        let error = format!("{:#}", parse(&newer).unwrap_err());
        assert!(error.contains("newer RustCode"), "{error}");
    }
}
