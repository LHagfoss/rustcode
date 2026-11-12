//! Saved machines and their routes.
//!
//! A machine is a computer this client has paired with, keyed by its
//! `host_id` (the remote gateway's stable identifier). Each one keeps an
//! ordered list of routes to reach it by, so a laptop that answers on the LAN
//! at home and over an overlay away from home is one entry, not two.
//!
//! The registry is one private JSON file in the configuration directory,
//! `machines.json` (`0600` on Unix, in a `0700` directory). Device tokens are
//! not in it; see [`crate::tokens`].
//!
//! Every change happens under an exclusive lock on `machines.lock` and
//! re-reads the file first, so two `rustcode machines` processes (or the app
//! and the command line) cannot drop each other's machine.

use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// Bounds, so a corrupted or hostile file cannot exhaust memory.
const MAX_MACHINES_BYTES: u64 = 256 * 1024;
const MAX_ROUTES_PER_MACHINE: usize = 16;
/// Shortest identifier prefix `remove` and `select` accept, matching
/// `rustcode remote revoke`.
const MIN_SELECTOR_CHARS: usize = 4;

/// How this client reaches a machine. Ordered as the app prefers to try them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    /// An address on the local network.
    Lan,
    /// A private overlay or VPN address (WireGuard, NetBird, …).
    Vpn,
    /// A Tailscale address or `*.ts.net` name.
    Tailscale,
    /// A public host name or address.
    Public,
}

impl RouteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lan => "lan",
            Self::Vpn => "vpn",
            Self::Tailscale => "tailscale",
            Self::Public => "public",
        }
    }

    /// Whether traffic over this route is encrypted by the network itself
    /// rather than plain on the LAN.
    pub fn is_encrypted(self) -> bool {
        matches!(self, Self::Vpn | Self::Tailscale)
    }

    /// The race order: LAN first, then an overlay, then a public route.
    pub fn rank(self) -> u8 {
        match self {
            Self::Lan => 0,
            Self::Vpn | Self::Tailscale => 1,
            Self::Public => 2,
        }
    }
}

impl fmt::Display for RouteKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.pad(self.as_str())
    }
}

/// The mesh product a route belongs to, when the machine's handshake said
/// which interface the address is on. The address range `100.64.0.0/10` is
/// shared by NetBird, Tailscale and other overlays, so only the machine can
/// say which one it is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlayProvider {
    Netbird,
    Tailscale,
    Zerotier,
}

impl OverlayProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Netbird => "netbird",
            Self::Tailscale => "tailscale",
            Self::Zerotier => "zerotier",
        }
    }

    /// The product's name as a person writes it, for a hint.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Netbird => "NetBird",
            Self::Tailscale => "Tailscale",
            Self::Zerotier => "ZeroTier",
        }
    }

    /// The `provider` a gateway reports, or `None` for a spelling this client
    /// does not know.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "netbird" => Some(Self::Netbird),
            "tailscale" => Some(Self::Tailscale),
            "zerotier" => Some(Self::Zerotier),
            _ => None,
        }
    }
}

impl std::fmt::Display for OverlayProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One way to reach a machine: `address` is what a WebSocket dials, a
/// `host:port`, a bare host (the default port), or `host:port`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    pub kind: RouteKind,
    pub address: String,
    /// Which mesh product the address is on, when the machine named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<OverlayProvider>,
}

impl Route {
    /// A route to `address`, with the kind its address implies.
    pub fn new(address: impl Into<String>) -> Self {
        let address = address.into();
        let kind = classify_address(&address);
        Self {
            kind,
            address,
            provider: provider_for_kind(kind),
        }
    }

    pub fn with_kind(kind: RouteKind, address: impl Into<String>) -> Self {
        Self {
            kind,
            address: address.into(),
            provider: provider_for_kind(kind),
        }
    }

    /// A route learned from a handshake `routes` entry: the kind and product
    /// the machine reported, so the same address is dialled and named the
    /// same way everywhere.
    pub fn reported(address: impl Into<String>, kind: &str, provider: Option<&str>) -> Self {
        let address = address.into();
        let provider = provider.and_then(OverlayProvider::from_wire);
        match kind {
            "lan" => Self::with_kind(RouteKind::Lan, address),
            // `vpn` is what a gateway before the overlay kind reported.
            "overlay" | "vpn" => {
                let provider = provider.or_else(|| match classify_address(&address) {
                    RouteKind::Tailscale => Some(OverlayProvider::Tailscale),
                    _ => None,
                });
                let kind = match provider {
                    Some(OverlayProvider::Tailscale) => RouteKind::Tailscale,
                    _ => RouteKind::Vpn,
                };
                Self {
                    kind,
                    address,
                    provider,
                }
            }
            _ => Self {
                provider,
                ..Self::new(address)
            },
        }
    }

    /// The name of the network this route is on, for a person: the product
    /// when known, otherwise the kind.
    pub fn network_name(&self) -> String {
        match self.provider {
            Some(provider) => provider.display_name().to_owned(),
            None if self.kind == RouteKind::Tailscale => "Tailscale".to_owned(),
            None if self.kind == RouteKind::Vpn => "the VPN".to_owned(),
            None => "the LAN".to_owned(),
        }
    }
}

fn provider_for_kind(kind: RouteKind) -> Option<OverlayProvider> {
    (kind == RouteKind::Tailscale).then_some(OverlayProvider::Tailscale)
}

/// Which kind an address implies without asking the network or the machine.
///
/// A `100.64.0.0/10` address is reported as [`RouteKind::Vpn`]: that range is
/// shared by Tailscale and other carriers of private addresses, and only the
/// machine itself can say which interface it is on (step 1.7 of the machine
/// network plan reports that; this is the seam it fills).
pub fn classify_address(address: &str) -> RouteKind {
    let host = crate::transport::host_part(address);
    let name = host.strip_suffix('.').unwrap_or(host);
    if !name.is_empty() && name.parse::<IpAddr>().is_err() {
        let name = name.to_ascii_lowercase();
        if name.ends_with(".ts.net") || name == "ts.net" {
            return RouteKind::Tailscale;
        }
        if name.ends_with(".local") || !name.contains('.') {
            return RouteKind::Lan;
        }
        return RouteKind::Public;
    }
    match name.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            let [a, b, ..] = ip.octets();
            if ip.is_loopback() || ip.is_private() || ip.is_link_local() {
                RouteKind::Lan
            } else if a == 100 && (64..128).contains(&b) {
                RouteKind::Vpn
            } else {
                RouteKind::Public
            }
        }
        Ok(IpAddr::V6(ip)) => {
            let segments = ip.segments();
            // Tailscale's IPv6 prefix, `fd7a:115c:a1e0::/48`.
            if segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0 {
                RouteKind::Tailscale
            } else if ip.is_loopback() || (segments[0] & 0xffc0) == 0xfe80 {
                RouteKind::Lan
            } else if (segments[0] & 0xfe00) == 0xfc00 {
                // Unique-local addressing, which a VPN hands out.
                RouteKind::Vpn
            } else {
                RouteKind::Public
            }
        }
        Err(_) => RouteKind::Public,
    }
}

/// Whether a credential may travel to `address` on the plain `ws` transport
/// this client speaks.
///
/// The gateway has no TLS listener, so a token sent to a public address is a
/// token handed to the network. A LAN or overlay route is the same risk the
/// phone app already accepts; a public one is not, and waits for encrypted
/// transport (#2651).
pub fn may_send_secrets(address: &str) -> bool {
    classify_address(address) != RouteKind::Public
}

/// The error for a route that may not carry a credential, naming the reason
/// and the issue that removes the restriction.
pub fn plain_ws_refusal(address: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "refusing to dial {address}: it is a public address and the gateway has no TLS listener, \
         so the pairing code and the device token would travel in the clear. Dial a LAN, \
         Tailscale or VPN address instead; encrypted transport is tracked in #2651"
    )
}

/// What a TCP connect to one route found, the last time it was tried.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeOutcome {
    /// The address accepted a connection.
    Reachable,
    /// Something answered and turned the connection away: the host is there,
    /// nothing listens on that port, or a firewall rejects it.
    Refused,
    /// Nothing answered in time.
    Timeout,
    /// The connect failed some other way: no route to the host, the name did
    /// not resolve.
    Unreachable,
}

impl ProbeOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::Refused => "refused",
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
        }
    }
}

/// The last probe of one route: what it found and how long it took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteProbe {
    /// The route's address, as saved.
    pub address: String,
    pub outcome: ProbeOutcome,
    /// Milliseconds to connect, or to be turned away; none for a timeout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    /// The error text of a connect that failed other than by timing out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub at: DateTime<Utc>,
}

/// A computer this client has paired with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    /// The remote gateway's stable identifier, as its `paired` frame reports
    /// it. The key of the entry, and what a route must authenticate against.
    pub host_id: String,
    /// The name the machine reports, or the one given with `--name`.
    pub name: String,
    /// A name only this app shows, in place of the reported one. Never sent
    /// to the machine, so renaming one here does not rename it there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The glyph this app draws for the machine, from its fixed icon set.
    /// `None` means the platform's own glyph.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// A colour tint for the glyph, from the app's series palette.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_tint: Option<String>,
    /// What the machine runs, when it says: `macos`, `windows`, `linux`, …
    /// `unknown` for a gateway that does not report it yet.
    #[serde(default = "unknown_platform")]
    pub platform: String,
    /// Routes in the order they are tried.
    pub routes: Vec<Route>,
    /// The RustCode version that machine reported when it last answered.
    /// `None` until it does, and from a gateway that does not report one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The device record on that machine, needed for `authenticate`.
    pub device_id: String,
    pub added_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected_at: Option<DateTime<Utc>>,
    /// The route that answered last time, tried first after a drop. `None`
    /// until the machine has been reached once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_route: Option<String>,
    /// What the last test of each route found. Rebuilt whenever the routes
    /// are probed; an entry for a route that was removed is dropped with it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<RouteProbe>,
    /// New threads are not started here from this app, though its threads
    /// still open and the connection stays up. A local choice, never sent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
}

fn unknown_platform() -> String {
    "unknown".to_owned()
}

impl Machine {
    /// A newly paired machine with one route.
    pub fn new(
        host_id: impl Into<String>,
        name: impl Into<String>,
        platform: impl Into<String>,
        device_id: impl Into<String>,
        route: Route,
        now: DateTime<Utc>,
    ) -> Self {
        let mut machine = Self {
            host_id: host_id.into(),
            name: name.into(),
            display_name: None,
            icon: None,
            icon_tint: None,
            platform: platform.into(),
            routes: vec![route],
            version: None,
            device_id: device_id.into(),
            added_at: now,
            last_connected_at: None,
            last_route: None,
            probes: Vec::new(),
            paused: false,
        };
        machine.normalize_routes();
        machine
    }

    /// The name this app shows: the local display name when one is set, else
    /// the name the machine reports.
    pub fn display(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    /// The route in use: the first one configured. Which one actually answers
    /// is decided per connection; keeping the list ordered is what makes the
    /// order meaningful (the full race, learn and move-back rules are the
    /// machine network's step 1.8).
    pub fn preferred_route(&self) -> Option<&Route> {
        self.routes.first()
    }

    /// The route to show for a machine that is not connected now: the one it
    /// answered on last, while it is still saved, else the first one. The
    /// first saved route is often a LAN address of a network this computer has
    /// left, which tells a person nothing about where the machine was reached.
    pub fn last_working_route(&self) -> Option<&Route> {
        self.last_route
            .as_deref()
            .and_then(|last| self.routes.iter().find(|route| route.address == last))
            .or_else(|| self.preferred_route())
    }

    /// Add a route unless the same address is already there, which only
    /// learns its kind and product. A learned route keeps the order the races
    /// rely on: it goes after the routes of its own class, so a LAN address
    /// learned later is still tried before an overlay one.
    pub(crate) fn learn_route(&mut self, mut route: Route) -> bool {
        route.address = explicit_overlay_address(&route);
        if let Some(existing) = self
            .routes
            .iter_mut()
            .find(|existing| same_address(&existing.address, &route.address))
        {
            let mut changed = false;
            if existing.kind != route.kind {
                existing.kind = route.kind;
                changed = true;
            }
            // A VPN route saved before ports were spelled out gets its port.
            let explicit = explicit_overlay_address(existing);
            if existing.address != explicit {
                existing.address = explicit;
                changed = true;
            }
            if route.provider.is_some() && existing.provider != route.provider {
                existing.provider = route.provider;
                changed = true;
            }
            return changed;
        }
        if self.routes.len() >= MAX_ROUTES_PER_MACHINE {
            return false;
        }
        let rank = route.kind.rank();
        let index = self
            .routes
            .iter()
            .rposition(|existing| existing.kind.rank() <= rank)
            .map_or(0, |last| last + 1);
        self.routes.insert(index, route);
        true
    }
}

impl Machine {
    /// Spell out the port of every overlay route, so a VPN route reads like the
    /// LAN ones (`100.90.16.237:17879`) and a probe or a hint names the port it
    /// used. Returns whether anything changed.
    fn normalize_routes(&mut self) -> bool {
        let mut changed = false;
        for route in &mut self.routes {
            let explicit = explicit_overlay_address(route);
            if route.address != explicit {
                if self
                    .last_route
                    .as_deref()
                    .is_some_and(|last| same_address(last, &route.address))
                {
                    self.last_route = Some(explicit.clone());
                }
                route.address = explicit;
                changed = true;
            }
        }
        changed
    }

    /// What the last test of `address` found, if it was tested.
    pub fn probe_for(&self, address: &str) -> Option<&RouteProbe> {
        self.probes
            .iter()
            .find(|probe| same_address(&probe.address, address))
    }
}

/// A VPN or Tailscale route's address with its port spelled out; any other
/// route's address as it is.
fn explicit_overlay_address(route: &Route) -> String {
    if matches!(route.kind, RouteKind::Vpn | RouteKind::Tailscale) {
        crate::transport::with_explicit_port(&route.address)
    } else {
        route.address.clone()
    }
}

/// The same endpoint spelled `studio.local:17879` and `studio.local`.
fn same_address(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right) || dial_host(left) == dial_host(right)
}

fn dial_host(address: &str) -> String {
    format!(
        "{}:{}",
        crate::transport::host_part(address).to_ascii_lowercase(),
        crate::transport::port_or_default(address)
    )
}

/// The saved machines, loaded once and written through.
#[derive(Debug)]
pub struct MachineRegistry {
    path: PathBuf,
    machines: Vec<Machine>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Stored {
    version: u32,
    #[serde(default)]
    machines: Vec<Machine>,
}

const STORE_VERSION: u32 = 1;

impl MachineRegistry {
    /// Load the machines in `config_dir`. A missing file is no machines; a
    /// file that cannot be read is an error, never an empty list that the
    /// next save would overwrite.
    pub fn load(config_dir: &Path) -> Result<Self> {
        Self::load_file(crate::config::machines_file(config_dir))
    }

    pub fn load_file(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let stored = read_stored(&path)?;
        Ok(Self {
            path,
            machines: stored.machines,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Oldest first. Every machine is a peer, so none is listed ahead of the others.
    pub fn machines(&self) -> &[Machine] {
        &self.machines
    }

    pub fn is_empty(&self) -> bool {
        self.machines.is_empty()
    }

    pub fn get(&self, host_id: &str) -> Option<&Machine> {
        self.machines
            .iter()
            .find(|machine| machine.host_id == host_id)
    }

    /// The machine `selector` names: its full `host_id`, an unambiguous
    /// prefix of one, or its unique name.
    pub fn select(&self, selector: &str) -> Result<&Machine> {
        let selector = selector.trim();
        ensure!(!selector.is_empty(), "name a machine to select");
        if let Some(machine) = self.get(selector) {
            return Ok(machine);
        }
        let named: Vec<&Machine> = self
            .machines
            .iter()
            .filter(|machine| machine.name.eq_ignore_ascii_case(selector))
            .collect();
        match named.len() {
            1 => return Ok(named[0]),
            2.. => bail!(
                "{} machines are named {selector}; use a host id instead",
                named.len()
            ),
            _ => {}
        }
        if selector.len() >= MIN_SELECTOR_CHARS {
            let matching: Vec<&Machine> = self
                .machines
                .iter()
                .filter(|machine| machine.host_id.starts_with(selector))
                .collect();
            match matching.len() {
                1 => return Ok(matching[0]),
                2.. => bail!("{selector} is a prefix of {} machines", matching.len()),
                _ => {}
            }
        }
        bail!("no saved machine is named {selector}{}", known_names(self));
    }

    /// Insert a newly paired machine, or replace what a re-pairing learned
    /// about a saved one: the name, platform, device record and routes,
    /// keeping when it was first added.
    pub fn insert(&mut self, machine: Machine) -> Result<()> {
        self.mutate(|machines| {
            match machines
                .iter_mut()
                .find(|existing| existing.host_id == machine.host_id)
            {
                Some(existing) => {
                    existing.name = machine.name;
                    existing.platform = machine.platform;
                    if machine.version.is_some() {
                        existing.version = machine.version;
                    }
                    existing.device_id = machine.device_id;
                    for route in machine.routes {
                        existing.learn_route(route);
                    }
                }
                None => machines.push(machine),
            }
            Ok(())
        })
    }

    /// Forget a machine. Returns the record that was removed, so the caller
    /// can drop its token too.
    pub fn remove(&mut self, selector: &str) -> Result<Machine> {
        let host_id = self.select(selector)?.host_id.clone();
        self.mutate(|machines| {
            let Some(index) = machines
                .iter()
                .position(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            Ok(machines.remove(index))
        })
    }

    /// Record a successful connection over `route`, so the row can say when
    /// the machine was last reached and the next attempt can try the route
    /// that worked first (#3436).
    pub fn mark_connected(&mut self, host_id: &str, route: &str, now: DateTime<Utc>) -> Result<()> {
        self.mutate(|machines| {
            if let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            {
                machine.last_connected_at = Some(now);
                machine.last_route = Some(route.to_owned());
            }
            Ok(())
        })
    }

    /// Record the version a machine reported, so its row can say it needs
    /// updating even while the machine is away. Returns whether it changed.
    pub fn set_version(&mut self, host_id: &str, version: Option<&str>) -> Result<bool> {
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            let version = version.map(str::to_owned);
            if machine.version == version {
                return Ok(false);
            }
            machine.version = version;
            Ok(true)
        })
    }

    /// Set the name this app shows for a machine. A blank name clears it, so
    /// the reported name is shown again. It never changes what the machine
    /// calls itself, and pairing again keeps it.
    pub fn set_display_name(&mut self, host_id: &str, display_name: Option<&str>) -> Result<()> {
        let display_name = display_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            machine.display_name = display_name;
            Ok(())
        })
    }

    /// Set the glyph this app draws for a machine, and its tint. Both `None`
    /// returns to the platform's own glyph. Pairing again keeps the choice.
    pub fn set_icon(
        &mut self,
        host_id: &str,
        icon: Option<&str>,
        tint: Option<&str>,
    ) -> Result<()> {
        let icon = icon.map(str::trim).filter(|icon| !icon.is_empty());
        let tint = icon
            .and_then(|_| tint.map(str::trim).filter(|tint| !tint.is_empty()))
            .map(str::to_owned);
        let icon = icon.map(str::to_owned);
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            machine.icon = icon;
            machine.icon_tint = tint;
            Ok(())
        })
    }

    /// Record the platform a machine reported, so a machine paired before its
    /// gateway sent one stops reading `unknown` (#3418). `None` — an older
    /// gateway that says nothing — keeps what is saved. Returns whether it
    /// changed.
    pub fn set_platform(&mut self, host_id: &str, platform: Option<&str>) -> Result<bool> {
        let Some(platform) = platform.filter(|platform| !platform.is_empty()) else {
            return Ok(false);
        };
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            if machine.platform == platform {
                return Ok(false);
            }
            machine.platform = platform.to_owned();
            Ok(true)
        })
    }

    /// Learn the addresses a machine reports after authentication, saving
    /// them for the next connection. Returns how many routes changed or were
    /// added; an unknown `host_id` is an error rather than a silent no-op.
    pub fn learn_routes(&mut self, host_id: &str, routes: &[Route]) -> Result<usize> {
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            Ok(routes
                .iter()
                .filter(|route| machine.learn_route((*route).clone()))
                .count())
        })
    }

    /// Add a route learned from a machine or a discovery record.
    pub fn add_route(&mut self, host_id: &str, route: Route) -> Result<bool> {
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            Ok(machine.learn_route(route))
        })
    }

    /// Save what a test of the machine's routes found, replacing the last one.
    /// Only a route the machine still has keeps its entry.
    pub fn set_probes(&mut self, host_id: &str, probes: Vec<RouteProbe>) -> Result<()> {
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            let routes = &machine.routes;
            machine.probes = probes
                .into_iter()
                .filter(|probe| {
                    routes
                        .iter()
                        .any(|route| same_address(&route.address, &probe.address))
                })
                .collect();
            Ok(())
        })
    }

    /// Pause or resume new threads on a machine. Its connection and its
    /// existing threads are untouched; only **Run on** refuses it.
    pub fn set_paused(&mut self, host_id: &str, paused: bool) -> Result<()> {
        self.mutate(|machines| {
            let Some(machine) = machines
                .iter_mut()
                .find(|machine| machine.host_id == host_id)
            else {
                bail!("no saved machine has host id {host_id}");
            };
            machine.paused = paused;
            Ok(())
        })
    }

    /// The machines in the order they are shown: oldest first.
    pub fn ordered(&self) -> Vec<&Machine> {
        let mut machines: Vec<&Machine> = self.machines.iter().collect();
        machines.sort_by_key(|machine| machine.added_at);
        machines
    }

    /// Change the registry under its write lock, re-reading the file first so
    /// a change made by another process since this copy was loaded is kept.
    fn mutate<T>(&mut self, change: impl FnOnce(&mut Vec<Machine>) -> Result<T>) -> Result<T> {
        let _lock = self.write_lock()?;
        self.machines = read_stored(&self.path)?.machines;
        let changed = change(&mut self.machines)?;
        self.save_unlocked()?;
        Ok(changed)
    }

    /// Write the registry atomically and privately.
    pub fn save(&self) -> Result<()> {
        let _lock = self.write_lock()?;
        self.save_unlocked()
    }

    fn save_unlocked(&self) -> Result<()> {
        let stored = Stored {
            version: STORE_VERSION,
            machines: self.machines.clone(),
        };
        write_private_json(&self.path, &stored)
    }

    /// The exclusive lock around load→change→save, held until the returned
    /// file is dropped. `machines.lock`, next to the registry.
    fn write_lock(&self) -> Result<File> {
        let directory = self
            .path
            .parent()
            .context("the machine registry needs a parent directory")?;
        create_private_directory(directory)?;
        let path = self.path.with_extension("lock");
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        file.lock()
            .with_context(|| format!("lock {}", path.display()))?;
        Ok(file)
    }
}

fn known_names(registry: &MachineRegistry) -> String {
    match registry.machines.as_slice() {
        [] => String::new(),
        machines => format!(
            "; saved: {}",
            machines
                .iter()
                .map(|machine| machine.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The registry as stored: no file is no machines, and a file that cannot be
/// read is an error rather than an empty list a write would overwrite.
fn read_stored(path: &Path) -> Result<Stored> {
    let Some(bytes) = read_bounded(path)? else {
        return Ok(Stored {
            version: STORE_VERSION,
            machines: Vec::new(),
        });
    };
    let mut stored: Stored = serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a machine registry", path.display()))?;
    ensure!(
        stored.version <= STORE_VERSION,
        "{} was written by a newer RustCode (format {}); update this one",
        path.display(),
        stored.version
    );
    // A file written before VPN routes carried a port is read with it, and
    // written that way by the next change.
    for machine in &mut stored.machines {
        machine.normalize_routes();
    }
    Ok(stored)
}

/// Read at most [`MAX_MACHINES_BYTES`]; `None` when the file does not exist.
fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_MACHINES_BYTES + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_MACHINES_BYTES,
        "{} is larger than {MAX_MACHINES_BYTES} bytes",
        path.display()
    );
    Ok(Some(bytes))
}

/// Replace `path` with complete owner-only JSON. The temporary file is
/// created `0600` in the same directory, so the destination never exists with
/// wider permissions or partial contents.
pub(crate) fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .context("a private file needs a parent directory")?;
    create_private_directory(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|error| error.error)
        .context("replace the file")?;
    Ok(())
}

fn create_private_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(directory)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(directory)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(dir: &Path) -> MachineRegistry {
        MachineRegistry::load(dir).unwrap()
    }

    fn machine(host_id: &str, name: &str, address: &str) -> Machine {
        Machine::new(
            host_id,
            name,
            "macos",
            "device-1",
            Route::new(address),
            Utc::now(),
        )
    }

    #[test]
    fn a_saved_machine_survives_a_reload() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        assert!(store.is_empty());
        store
            .insert(machine("gw-8e1f4b7a", "Studio", "192.168.1.20:17879"))
            .unwrap();
        let reloaded = load(dir.path());
        assert_eq!(reloaded.machines().len(), 1);
        let studio = &reloaded.machines()[0];
        assert_eq!(studio.host_id, "gw-8e1f4b7a");
        assert_eq!(studio.name, "Studio");
        assert_eq!(studio.platform, "macos");
        assert_eq!(studio.device_id, "device-1");
        assert_eq!(
            studio.routes,
            [Route::with_kind(RouteKind::Lan, "192.168.1.20:17879")]
        );
        assert!(studio.last_connected_at.is_none());
        assert_eq!(reloaded.path(), crate::config::machines_file(dir.path()));
    }

    /// The file is `0600`, and a directory this crate creates is `0700`. A
    /// directory that already exists (the user's configuration directory) is
    /// left as it is: widening or narrowing someone else's directory is not
    /// this crate's to decide, and the file inside is owner-only either way.
    #[cfg(unix)]
    #[test]
    fn the_registry_is_owner_only() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let config = dir.path().join("new-config");
        let mut store = MachineRegistry::load(&config).unwrap();
        store
            .insert(machine("gw-a", "A", "10.0.0.2:17879"))
            .unwrap();
        let metadata = std::fs::metadata(store.path()).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        let directory = std::fs::metadata(&config).unwrap();
        assert_eq!(directory.mode() & 0o777, 0o700);
        // An existing directory keeps its own mode.
        let mut store = MachineRegistry::load(dir.path()).unwrap();
        store
            .insert(machine("gw-b", "B", "10.0.0.3:17879"))
            .unwrap();
        let mode = std::fs::metadata(store.path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn removing_selects_by_name_prefix_or_id_and_drops_its_routes() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("3f9a1c7e5b2d4086", "Studio", "10.0.0.2:17879"))
            .unwrap();
        store
            .insert(machine("aabbccddeeff0011", "Laptop", "10.0.0.3:17879"))
            .unwrap();
        assert_eq!(store.select("Studio").unwrap().host_id, "3f9a1c7e5b2d4086");
        assert_eq!(store.select("studio").unwrap().host_id, "3f9a1c7e5b2d4086");
        assert_eq!(store.select("aabb").unwrap().host_id, "aabbccddeeff0011");
        assert!(store.select("a").is_err(), "one character is ambiguous");
        assert!(store.select("nope").is_err());
        let removed = store.remove("Studio").unwrap();
        assert_eq!(removed.host_id, "3f9a1c7e5b2d4086");
        assert_eq!(load(dir.path()).machines().len(), 1);
        assert_eq!(store.machines()[0].name, "Laptop");
        assert!(store.remove("Studio").is_err());
    }

    /// A file from when one machine was "main" still loads: the flag is read
    /// and dropped, no machine is listed ahead of the others, and the next
    /// write does not bring it back.
    #[test]
    fn an_old_main_flag_is_ignored_and_not_written_back() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("gw-a", "A", "10.0.0.2:17879"))
            .unwrap();
        store
            .insert(machine("gw-b", "B", "10.0.0.3:17879"))
            .unwrap();
        let path = crate::config::machines_file(dir.path());
        let mut stored: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        stored["machines"][0]["main"] = serde_json::json!(false);
        stored["machines"][1]["main"] = serde_json::json!(true);
        std::fs::write(&path, stored.to_string()).unwrap();
        let mut old = load(dir.path());
        assert_eq!(
            old.ordered()
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>(),
            ["A", "B"]
        );
        old.set_paused("gw-a", true).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("\"main\""), "{written}");
    }

    #[test]
    fn pairing_again_updates_in_place_and_learns_routes() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        let first = machine("gw-a", "Studio", "192.168.1.20:17879");
        let added_at = first.added_at;
        store.insert(first).unwrap();
        store
            .add_route(
                "gw-a",
                Route::with_kind(RouteKind::Vpn, "100.92.13.44:17879"),
            )
            .unwrap();
        // The same address with another port is another endpoint to try.
        assert!(
            store
                .add_route(
                    "gw-a",
                    Route::with_kind(RouteKind::Lan, "192.168.1.20:9000")
                )
                .unwrap()
        );
        // Learning the same address again changes nothing.
        assert!(
            !store
                .add_route(
                    "gw-a",
                    Route::with_kind(RouteKind::Lan, "192.168.1.20:9000")
                )
                .unwrap()
        );
        let mut renamed = machine("gw-a", "Studio (MacBook)", "192.168.1.20:17879");
        renamed.added_at = Utc::now() + chrono::Duration::hours(1);
        store.insert(renamed).unwrap();
        let reloaded = load(dir.path());
        let studio = &reloaded.machines()[0];
        assert_eq!(studio.name, "Studio (MacBook)");
        assert_eq!(studio.added_at, added_at, "first pairing keeps its date");
        assert_eq!(reloaded.machines().len(), 1);
        assert_eq!(studio.routes.len(), 3);
        assert_eq!(studio.preferred_route().unwrap().kind, RouteKind::Lan);
    }

    /// A file written before the display name and icon existed still loads,
    /// and a rename and icon survive a reload. A blank name clears the local
    /// name, and pairing the same machine again keeps both choices.
    #[test]
    fn a_display_name_and_icon_round_trip_and_old_files_still_load() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let path = crate::config::machines_file(dir.path());
        // What an older build wrote: no display_name, icon or icon_tint.
        std::fs::write(
            &path,
            br#"{"version": 1, "machines": [{"host_id": "gw-old", "name": "Studio", "platform": "macos", "routes": [{"kind": "lan", "address": "10.0.0.2:17879"}], "device_id": "device-1", "added_at": "2026-01-02T03:04:05Z"}]}"#,
        )
        .unwrap();
        let mut store = load(dir.path());
        let studio = &store.machines()[0];
        assert_eq!(studio.name, "Studio");
        assert_eq!(studio.display_name, None);
        assert_eq!(studio.icon, None);
        assert_eq!(studio.display(), "Studio");

        store
            .set_display_name("gw-old", Some("  Studio desk  "))
            .unwrap();
        store.set_icon("gw-old", Some("studio"), Some("4")).unwrap();
        let reloaded = load(dir.path());
        let studio = &reloaded.machines()[0];
        assert_eq!(studio.name, "Studio");
        assert_eq!(studio.display_name.as_deref(), Some("Studio desk"));
        assert_eq!(studio.display(), "Studio desk");
        assert_eq!(studio.icon.as_deref(), Some("studio"));
        assert_eq!(studio.icon_tint.as_deref(), Some("4"));
        // Old files have no pause flag either, and it round-trips when set.
        assert!(!studio.paused);
        store.set_paused("gw-old", true).unwrap();
        let paused = load(dir.path());
        assert!(paused.machines()[0].paused);
        // A re-pair keeps the local name and icon: they are this app's, not the
        // machine's, and pairing again only refreshes what the machine reports.
        let mut repaired = machine("gw-old", "Studio (reported)", "10.0.0.3:17879");
        repaired.display_name = None;
        repaired.icon = None;
        reloaded_register(dir.path(), repaired);
        let repaired_store = load(dir.path());
        let studio = &repaired_store.machines()[0];
        assert_eq!(studio.name, "Studio (reported)");
        assert_eq!(studio.display_name.as_deref(), Some("Studio desk"));
        assert_eq!(studio.icon.as_deref(), Some("studio"));

        // A blank name (or an icon cleared) returns to the reported name and
        // the platform's own glyph.
        let mut store = load(dir.path());
        store.set_display_name("gw-old", Some("   ")).unwrap();
        store.set_icon("gw-old", None, Some("4")).unwrap();
        let studio = &store.machines()[0];
        assert_eq!(studio.display_name, None);
        assert_eq!(studio.display(), "Studio (reported)");
        assert_eq!(studio.icon, None);
        assert_eq!(studio.icon_tint, None);
        // The fields are left out of the file when unset, so an older build
        // reads a file this one wrote.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("display_name"), "{text}");
        assert!(!text.contains("icon"), "{text}");
    }

    fn reloaded_register(dir: &std::path::Path, machine: Machine) {
        let mut store = load(dir);
        store.insert(machine).unwrap();
    }

    /// A machine paired while its gateway reported no platform learns it on
    /// the next connection, and stays `unknown` while the gateway is older
    /// (#3418).
    #[test]
    fn a_reported_platform_refreshes_a_saved_machine() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("gw-a", "Studio", "192.168.1.20:17879"))
            .unwrap();
        store.machines[0].platform = "unknown".into();
        // An older gateway says nothing: what is saved stays.
        assert!(!store.set_platform("gw-a", None).unwrap());
        assert_eq!(store.machines()[0].platform, "unknown");
        // A newer one reports its platform, which is saved.
        assert!(store.set_platform("gw-a", Some("windows")).unwrap());
        assert_eq!(load(dir.path()).machines()[0].platform, "windows");
        // Reporting the same one again changes nothing.
        assert!(!store.set_platform("gw-a", Some("windows")).unwrap());
        assert!(store.set_platform("gw-missing", Some("linux")).is_err());
    }

    /// A successful connection records when it happened and over which route,
    /// so the next attempt can try the route that worked first (#3436).
    #[test]
    fn a_successful_connection_remembers_the_route_it_came_up_over() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("gw-a", "Studio", "192.168.1.20:17879"))
            .unwrap();
        store
            .add_route(
                "gw-a",
                Route::with_kind(RouteKind::Vpn, "100.90.16.237:17879"),
            )
            .unwrap();
        assert_eq!(store.machines()[0].last_route, None);
        let now = Utc::now();
        store
            .mark_connected("gw-a", "100.90.16.237:17879", now)
            .unwrap();
        let registry = load(dir.path());
        let studio = &registry.machines()[0];
        assert_eq!(studio.last_connected_at, Some(now));
        assert_eq!(studio.last_route.as_deref(), Some("100.90.16.237:17879"));
        // The saved routes are untouched: the order is the race's.
        assert_eq!(studio.preferred_route().unwrap().kind, RouteKind::Lan);
        // A machine that is away is shown on the route it answered on, not on
        // a LAN address of a network this computer left.
        assert_eq!(
            studio.last_working_route().unwrap().address,
            "100.90.16.237:17879"
        );
        let mut forgotten = studio.clone();
        forgotten.last_route = Some("10.9.9.9:17879".to_owned());
        assert_eq!(forgotten.last_working_route().unwrap().kind, RouteKind::Lan);
    }

    /// Routes a handshake reports are saved in the order the race uses, with
    /// the product when the machine named one, so a learned LAN address is
    /// still tried before the overlay the pairing came over.
    #[test]
    fn learned_routes_keep_the_race_order_and_the_provider() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("gw-a", "Studio", "100.90.16.237:17879"))
            .unwrap();
        let learned = [
            Route::reported("100.90.16.237:17879", "overlay", Some("netbird")),
            Route::reported("192.168.1.20:17879", "lan", None),
            // The same address twice is learned once.
            Route::reported("192.168.1.20:17879", "lan", None),
        ];
        assert_eq!(store.learn_routes("gw-a", &learned).unwrap(), 2);
        let reloaded = load(dir.path());
        let studio = &reloaded.machines()[0];
        assert_eq!(
            studio
                .routes
                .iter()
                .map(|route| (route.kind, route.address.as_str()))
                .collect::<Vec<_>>(),
            [
                (RouteKind::Lan, "192.168.1.20:17879"),
                (RouteKind::Vpn, "100.90.16.237:17879"),
            ]
        );
        assert_eq!(
            studio.routes[1].provider,
            Some(OverlayProvider::Netbird),
            "the product is kept, so a hint can name it"
        );
        assert!(store.learn_routes("gw-missing", &learned).is_err());
    }

    /// A VPN route is saved with its port like the LAN ones, and one saved
    /// without a port by an older release is read with it.
    #[test]
    fn a_vpn_route_is_stored_with_its_port() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("gw-a", "Studio", "192.168.1.20:17879"))
            .unwrap();
        store
            .add_route("gw-a", Route::with_kind(RouteKind::Vpn, "100.90.16.237"))
            .unwrap();
        store
            .add_route("gw-a", Route::with_kind(RouteKind::Vpn, "fdfd::2"))
            .unwrap();
        let reloaded = load(dir.path());
        let addresses: Vec<&str> = reloaded.machines()[0]
            .routes
            .iter()
            .map(|route| route.address.as_str())
            .collect();
        assert_eq!(
            addresses,
            [
                "192.168.1.20:17879",
                "100.90.16.237:17879",
                "[fdfd::2]:17879"
            ]
        );
        // Learning the same address with its port again changes nothing.
        assert!(
            !store
                .add_route(
                    "gw-a",
                    Route::with_kind(RouteKind::Vpn, "100.90.16.237:17879")
                )
                .unwrap()
        );
        // A file from before ports were spelled out reads with them.
        let path = crate::config::machines_file(dir.path());
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("100.90.16.237:17879", "100.90.16.237");
        std::fs::write(&path, text).unwrap();
        let old = load(dir.path());
        assert_eq!(old.machines()[0].routes[1].address, "100.90.16.237:17879");
    }

    #[test]
    fn route_probes_round_trip_and_follow_the_routes() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("gw-a", "Studio", "192.168.1.20:17879"))
            .unwrap();
        store
            .add_route("gw-a", Route::with_kind(RouteKind::Vpn, "100.90.16.237"))
            .unwrap();
        let probe = |address: &str, outcome, latency_ms| RouteProbe {
            address: address.to_owned(),
            outcome,
            latency_ms,
            detail: None,
            at: Utc::now(),
        };
        store
            .set_probes(
                "gw-a",
                vec![
                    probe("192.168.1.20:17879", ProbeOutcome::Reachable, Some(3)),
                    probe("100.90.16.237:17879", ProbeOutcome::Timeout, None),
                    probe("10.0.0.9:17879", ProbeOutcome::Refused, Some(1)),
                ],
            )
            .unwrap();
        let reloaded = load(dir.path());
        let studio = &reloaded.machines()[0];
        assert_eq!(
            studio.probes.len(),
            2,
            "a probe of a route it lacks is dropped"
        );
        assert_eq!(
            studio.probe_for("192.168.1.20").map(|p| p.outcome),
            Some(ProbeOutcome::Reachable),
            "a route is found by its endpoint, however the port is spelled"
        );
        assert_eq!(
            studio.probe_for("100.90.16.237:17879").map(|p| p.outcome),
            Some(ProbeOutcome::Timeout)
        );
        assert!(store.set_probes("gw-missing", Vec::new()).is_err());
    }

    /// The kinds a machine reports become this client's kinds, including the
    /// `vpn` an older gateway still sends and the product a newer one names.
    #[test]
    fn a_reported_route_keeps_the_machine_kind_and_product() {
        let netbird = Route::reported("100.90.16.237:17879", "overlay", Some("netbird"));
        assert_eq!(netbird.kind, RouteKind::Vpn);
        assert_eq!(netbird.provider, Some(OverlayProvider::Netbird));
        let tailscale = Route::reported("100.101.1.2:17879", "overlay", Some("tailscale"));
        assert_eq!(tailscale.kind, RouteKind::Tailscale);
        assert_eq!(tailscale.network_name(), "Tailscale");
        // An older gateway's `vpn` with a Tailscale address is still Tailscale.
        let legacy = Route::reported("mac.tail1234.ts.net:17879", "vpn", None);
        assert_eq!(legacy.kind, RouteKind::Tailscale);
        let lan = Route::reported("192.168.1.20:17879", "lan", None);
        assert_eq!(lan.kind, RouteKind::Lan);
        assert_eq!(lan.network_name(), "the LAN");
        // An unknown provider is dropped, not invented.
        let unknown = Route::reported("10.8.0.2:17879", "overlay", Some("hamachi"));
        assert_eq!(unknown.kind, RouteKind::Vpn);
        assert_eq!(unknown.provider, None);
        assert_eq!(unknown.network_name(), "the VPN");
    }

    #[test]
    fn an_unreadable_file_is_an_error_not_an_empty_registry() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        std::fs::write(crate::config::machines_file(dir.path()), b"{not json").unwrap();
        let error = MachineRegistry::load(dir.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains("machine registry"),
            "{error:#}"
        );
        std::fs::write(
            crate::config::machines_file(dir.path()),
            br#"{"version": 99, "machines": []}"#,
        )
        .unwrap();
        let error = MachineRegistry::load(dir.path()).unwrap_err();
        assert!(format!("{error:#}").contains("newer RustCode"), "{error:#}");
    }

    /// Two copies loaded before either writes must not drop each other's
    /// machine: each change re-reads the file under the lock.
    #[test]
    fn concurrent_changes_keep_every_machine() {
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut first = load(dir.path());
        let mut second = load(dir.path());
        assert!(first.machines().is_empty() && second.machines().is_empty());
        first
            .insert(machine("gw-a", "A", "10.0.0.2:17879"))
            .unwrap();
        second
            .insert(machine("gw-b", "B", "10.0.0.3:17879"))
            .unwrap();
        // The second writer's copy now holds both, and so does the file.
        assert_eq!(second.machines().len(), 2);
        let stored = load(dir.path());
        let mut names: Vec<&str> = stored
            .machines()
            .iter()
            .map(|machine| machine.name.as_str())
            .collect();
        names.sort();
        assert_eq!(names, ["A", "B"]);
        // A removal from a stale copy of the same file keeps the other one.
        let mut third = load(dir.path());
        assert!(third.machines().len() == 2);
        first.remove("A").unwrap();
        third.remove("B").unwrap();
        assert!(load(dir.path()).machines().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_registry_lock_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir_in("/tmp").unwrap();
        let mut store = load(dir.path());
        store
            .insert(machine("gw-a", "A", "10.0.0.2:17879"))
            .unwrap();
        let lock = store.path().with_extension("lock");
        assert_eq!(lock.file_name().unwrap(), "machines.lock");
        let mode = std::fs::metadata(lock).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    /// A credential may not travel to a public address on plain `ws`; a LAN,
    /// Tailscale or VPN route is the risk the phone app already accepts.
    #[test]
    fn only_non_public_routes_may_carry_a_credential() {
        for address in [
            "192.168.1.20:17879",
            "10.0.0.2",
            "127.0.0.1:17879",
            "[fd00::1]:17879",
            "100.92.13.44:17879",
            "mac.tail1234.ts.net",
            "studio.local",
        ] {
            assert!(may_send_secrets(address), "{address}");
        }
        for address in ["203.0.113.9:17879", "example.com", "studio.example.com"] {
            assert!(!may_send_secrets(address), "{address}");
            let refusal = plain_ws_refusal(address).to_string();
            assert!(refusal.contains(address), "{refusal}");
            assert!(refusal.contains("#2651"), "{refusal}");
            assert!(refusal.contains("no TLS listener"), "{refusal}");
        }
    }

    #[test]
    fn a_saved_public_route_is_described_as_public() {
        let public = machine("gw-a", "A", "203.0.113.9:17879");
        assert_eq!(public.routes[0].kind, RouteKind::Public);
        let overlay = machine("gw-b", "B", "100.92.13.44:17879");
        assert_eq!(overlay.routes[0].kind, RouteKind::Vpn);
    }

    #[test]
    fn routes_are_classified_by_address() {
        for (address, kind) in [
            ("192.168.1.20", RouteKind::Lan),
            ("10.0.0.2:17879", RouteKind::Lan),
            ("172.16.4.5:17879", RouteKind::Lan),
            ("127.0.0.1:17879", RouteKind::Lan),
            ("[::1]:17879", RouteKind::Lan),
            ("100.92.13.44:17879", RouteKind::Vpn),
            ("100.64.0.1", RouteKind::Vpn),
            ("100.128.0.1", RouteKind::Public),
            ("203.0.113.9:17879", RouteKind::Public),
            ("8.8.8.8", RouteKind::Public),
            ("mac.tail1234.ts.net", RouteKind::Tailscale),
            ("mac.tail1234.ts.net:17879", RouteKind::Tailscale),
            ("[fd7a:115c:a1e0::1]:17879", RouteKind::Tailscale),
            ("[fd00::1]:17879", RouteKind::Vpn),
            ("[2001:db8::20]:17879", RouteKind::Public),
            ("rustcode-8e1f4b7a.local.", RouteKind::Lan),
            ("studio.local", RouteKind::Lan),
            ("studio", RouteKind::Lan),
            ("example.com", RouteKind::Public),
            // The address a person types may still carry its scheme.
            ("ws://192.168.1.20:17879", RouteKind::Lan),
            ("ws://studio.local:17879/", RouteKind::Lan),
            ("wss://studio.example.com", RouteKind::Public),
        ] {
            assert_eq!(classify_address(address), kind, "{address}");
        }
    }

    #[test]
    fn a_route_behind_a_bracket_or_a_default_port_is_recognised_as_the_same() {
        assert!(same_address("studio.local", "studio.local:17879"));
        assert!(same_address("STUDIO.local:17879", "studio.local"));
        assert!(same_address("[fd00::1]:17879", "[fd00::1]"));
        assert!(!same_address("studio.local", "studio.local:9000"));
        assert!(!same_address("studio.local", "laptop.local"));
        assert!(!same_address("10.0.0.2", "10.0.0.3"));
    }
}
