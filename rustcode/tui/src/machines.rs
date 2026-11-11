//! `rustcode machines`: the other computers this one is paired with.
//!
//! Pairing is ask-to-connect: this end asks, the person at the other machine
//! accepts and reads an eight-digit code off it, this end types that code.
//! The token that comes back goes in the operating system credential store
//! and the machine goes into `machines.json`; see the `rustcode-client` crate.

use crate::cli::MachineCommands;
use anyhow::{Context, Result, bail, ensure};
use rustcode_client::machine::{APPROVAL_TIMEOUT, normalize_code, platform};
use rustcode_client::{
    ClientKind, Connector, DiscoveredMachine, Machine, MachineRegistry, PAIRING_CODE_DIGITS, Route,
    RouteFailure, Session, TokenStore, browse_async,
};
use std::io::{BufRead, Read, Write};
use std::time::Duration;

/// Longest code input read from standard input: eight digits plus the
/// separators and stray whitespace a paste carries. Anything longer is not a
/// code, and reading it all would be reading whatever the pipe holds.
const MAX_CODE_INPUT_BYTES: u64 = 64;

/// One saved machine and what asking it right now said.
struct Row {
    machine: Machine,
    route: String,
    state: State,
    /// What to check when one kind of route answers and another does not,
    /// e.g. a NetBird address that is blocked while the LAN works.
    hint: Option<String>,
    threads: Option<Vec<String>>,
}

#[derive(Clone, PartialEq, Eq)]
enum State {
    Connected,
    /// The machine did not answer on any route.
    Offline(String),
    /// It answered but refused the token: pair again.
    NeedsPairing(String),
    NoToken,
}

impl State {
    fn label(&self) -> String {
        match self {
            Self::Connected => "connected".to_owned(),
            Self::Offline(_) => "offline".to_owned(),
            Self::NeedsPairing(_) => "pairing expired".to_owned(),
            Self::NoToken => "not paired".to_owned(),
        }
    }

    fn detail(&self) -> Option<&str> {
        match self {
            Self::Offline(reason) | Self::NeedsPairing(reason) => Some(reason),
            _ => None,
        }
    }
}

pub async fn run(command: &MachineCommands) -> Result<()> {
    let config_dir = rustcode_client::config::config_dir()?;
    match command {
        MachineCommands::List { threads, json } => list(&config_dir, *threads, *json).await,
        MachineCommands::Add {
            address,
            name,
            code,
            discover,
            discover_secs,
        } => {
            add(
                &config_dir,
                address.as_deref(),
                name.clone(),
                code.clone(),
                *discover,
                Duration::from_secs(*discover_secs),
            )
            .await
        }
        MachineCommands::Remove { machine } => remove(&config_dir, machine),
        MachineCommands::Discover { secs, json } => {
            discover(Duration::from_secs(*secs), *json).await
        }
    }
}

/// Every address the machine answers on, joined: `list` prints one line per
/// machine, and the state says which one the connection came from.
fn route_list(machine: &Machine) -> String {
    machine
        .routes
        .iter()
        .map(|route| route.address.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

async fn list(config_dir: &std::path::Path, threads: bool, json: bool) -> Result<()> {
    let registry = MachineRegistry::load(config_dir)?;
    if registry.is_empty() {
        if json {
            println!("[]");
        } else {
            println!(
                "No machines saved. Add one with `rustcode machines add <address>`, or \
                 `rustcode machines discover` to find them on this network."
            );
        }
        return Ok(());
    }
    let tokens = TokenStore::open(config_dir)?;
    // `list` is a glance: a route that does not answer in seconds is skipped
    // instead of waited on.
    let connector = Connector::new(config_dir, ClientKind::Desktop)?
        .with_timeouts(Duration::from_secs(3), Duration::from_secs(5));
    let mut rows = Vec::new();
    // Routes and platforms learned from a handshake are saved after the run,
    // so the loop keeps borrowing the loaded registry.
    let mut learned: Vec<(String, Vec<Route>)> = Vec::new();
    let mut platforms: Vec<(String, String)> = Vec::new();
    // This computer's own interfaces, so a hint never asks about an overlay
    // this computer is not on (#3421).
    let local = rustcode_client::local_addresses();
    for machine in registry.ordered() {
        let mut row = Row {
            machine: machine.clone(),
            route: route_list(machine),
            state: State::NoToken,
            hint: None,
            threads: None,
        };
        if tokens.get(&machine.host_id)?.is_none() {
            // Nothing to present: do not dial a machine we cannot get into.
            rows.push(row);
            continue;
        }
        match connector.connect_raced(machine, &tokens).await {
            Ok(connected) => {
                row.route = connected.route.address.clone();
                row.state = State::Connected;
                if !connected.learned.is_empty() {
                    learned.push((machine.host_id.clone(), connected.learned.clone()));
                }
                // A machine paired while its gateway reported no platform
                // learns it now, so the row stops saying `unknown` (#3418).
                if let Some(platform) = connected.session.welcome().platform.clone() {
                    row.machine.platform = platform.clone();
                    platforms.push((machine.host_id.clone(), platform));
                }
                // A LAN win stops the race before the overlay attempts start,
                // so ask about them to say why they are not used (#3202); an
                // overlay win may instead leave a LAN failure in the race.
                let probe = connector
                    .probe_overlay_routes(machine, &connected.route)
                    .await;
                row.hint = hint(machine, &connected.route, connected.failed, probe, &local);
                if threads {
                    row.threads = Some(match read_threads(connected.session).await {
                        Ok(titles) => titles,
                        Err(error) => vec![format!("could not read threads: {error:#}")],
                    });
                }
            }
            Err(error) => {
                // A refusal that pairing again fixes reads as an expired
                // pairing; anything else (a timeout, a different machine at
                // that address) is offline.
                let needs_pairing = error
                    .downcast_ref::<rustcode_client::Unreachable>()
                    .is_some_and(rustcode_client::Unreachable::needs_new_pairing);
                let text = format!("{error:#}");
                row.state = if needs_pairing {
                    State::NeedsPairing(text)
                } else {
                    State::Offline(text)
                };
            }
        }
        rows.push(row);
    }
    if (!learned.is_empty() || !platforms.is_empty())
        && let Ok(mut registry) = MachineRegistry::load(config_dir)
    {
        for (host_id, routes) in learned {
            let _ = registry.learn_routes(&host_id, &routes);
        }
        for (host_id, platform) in platforms {
            let _ = registry.set_platform(&host_id, Some(&platform));
        }
    }
    if json {
        print_json(&rows);
    } else {
        print_rows(&rows);
    }
    Ok(())
}

async fn read_threads(session: Session) -> Result<Vec<String>> {
    let mut channel = session.acp().await?;
    rustcode_client::machine::thread_titles(&mut channel).await
}

/// The hint for a connected machine, from both the race's own failures and
/// the overlay probe's: a LAN win has no overlay failure yet, and an overlay
/// win keeps the LAN failure that explains why it is not on the local network.
///
/// `local` is this computer's own addresses: a route on an overlay without an
/// interface here can never answer, and a VPN route that is not needed while
/// another VPN route answers says nothing (#3421).
fn hint(
    machine: &Machine,
    route: &Route,
    in_race: Vec<RouteFailure>,
    probe: Vec<RouteFailure>,
    local: &[std::net::IpAddr],
) -> Option<String> {
    let mut failures = in_race;
    failures.extend(probe);
    rustcode_client::RouteReport {
        routes: &machine.routes,
        in_use: route,
        failures: &failures,
        local,
    }
    .hint()
}

fn print_rows(rows: &[Row]) {
    let mut table = vec![vec![
        "NAME".to_owned(),
        "PLATFORM".to_owned(),
        "ROUTE".to_owned(),
        "STATE".to_owned(),
    ]];
    for row in rows {
        table.push(vec![
            if row.machine.main {
                format!("{} (main)", row.machine.name)
            } else {
                row.machine.name.clone()
            },
            row.machine.platform.clone(),
            if row.route.is_empty() {
                "—".to_owned()
            } else {
                row.route.clone()
            },
            row.state.label(),
        ]);
    }
    println!("{}", columns(&table));
    for row in rows {
        for line in machine_lines(row) {
            println!("  {line}");
        }
    }
}

/// What is printed under the table for one machine: what happened to it, the
/// hint about a route of another kind, then its threads. The hint comes before
/// the threads so that `--threads` never prints it in the middle of a machine's
/// thread list (#3421).
fn machine_lines(row: &Row) -> Vec<String> {
    let mut lines: Vec<String> = row.state.detail().map(str::to_owned).into_iter().collect();
    lines.extend(row.hint.clone());
    lines.extend(row.threads.iter().flatten().cloned());
    lines
}

fn print_json(rows: &[Row]) {
    let machines: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            serde_json::json!({
                "name": row.machine.name,
                "host_id": row.machine.host_id,
                "platform": row.machine.platform,
                "device_id": row.machine.device_id,
                "main": row.machine.main,
                "route": row.route,
                "routes": row.machine.routes.iter().map(|route| serde_json::json!({
                    "kind": route.kind.as_str(),
                    "address": route.address,
                })).collect::<Vec<_>>(),
                "state": row.state.label(),
                "detail": row.state.detail(),
                "hint": row.hint,
                "added_at": row.machine.added_at,
                "last_connected_at": row.machine.last_connected_at,
                "threads": row.threads,
            })
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&machines).unwrap_or_default()
    );
}

/// Rows of cells as a plain table: two spaces between columns, the last one
/// unpadded so a terminal line has no trailing spaces.
fn columns(rows: &[Vec<String>]) -> String {
    let width = |index: usize| {
        rows.iter()
            .filter_map(|row| row.get(index))
            .map(|cell| cell.chars().count())
            .max()
            .unwrap_or(0)
    };
    let last = rows.iter().map(Vec::len).max().unwrap_or(1);
    let widths: Vec<usize> = (0..last).map(width).collect();
    rows.iter()
        .map(|row| {
            let mut line = String::new();
            for (index, cell) in row.iter().enumerate() {
                if index + 1 == row.len() {
                    line.push_str(cell);
                } else {
                    line.push_str(&format!("{cell:<width$}  ", width = widths[index]));
                }
            }
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn add(
    config_dir: &std::path::Path,
    address: Option<&str>,
    name: Option<String>,
    code: Option<String>,
    from_discovery: bool,
    discover_for: Duration,
) -> Result<()> {
    let address = match (address, from_discovery) {
        (Some(address), false) => address.to_owned(),
        (None, true) => pick_discovered(discover_for).await?,
        (Some(_), true) => unreachable!("clap refuses --discover with an address"),
        (None, false) => {
            bail!("name a machine address (`rustcode machines add <address>`) or pass --discover")
        }
    };
    let connector = Connector::new(config_dir, ClientKind::Desktop)?;
    // The credential store is opened before the machine is asked: a store that
    // cannot answer has to stop the pairing at this end, not after the other
    // machine has accepted the request and enrolled this computer (#3325).
    let tokens = std::sync::Arc::new(
        rustcode_client::tokens::open_bounded(config_dir, rustcode_client::tokens::STORE_TIMEOUT)
            .await?,
    );
    // Refuse a mistyped `--code` before asking the machine: the code has
    // eight digits, and the frame that reports its length arrives only after
    // the owner has accepted.
    let given = code
        .map(|code| parse_code(&code, PAIRING_CODE_DIGITS))
        .transpose()?;
    let label = name.clone().unwrap_or_else(|| address.clone());
    let ask = connector
        .ask(&address)
        .await
        .with_context(|| format!("could not ask {address} to connect"))?;
    println!(
        "Waiting for {label} to accept… (deny it on that machine to stop, or wait two minutes)"
    );
    let Some(approved) = ask.wait(APPROVAL_TIMEOUT).await? else {
        bail!("{label} did not answer the connection request; ask again");
    };
    let digits = approved.code_length();
    let code = match given {
        Some(code) if code.len() == digits as usize => code,
        Some(code) => parse_code(&code, digits)?,
        None => read_code(digits)?,
    };
    let paired = approved
        .pair(&code)
        .await
        .context("the code was not accepted; ask again and read the new code")?;
    let welcome = paired.session.welcome().clone();
    let machine = rustcode_client::machine_from_pairing(&welcome, Route::new(address), name);
    // Token first: a saved machine without one would only ever say "pair
    // again", while a token whose machine was not saved is unreachable.
    rustcode_client::tokens::set_bounded(
        std::sync::Arc::clone(&tokens),
        machine.host_id.clone(),
        paired.secret.clone(),
        rustcode_client::tokens::STORE_TIMEOUT,
    )
    .await
    .context("the machine is paired but its token could not be stored")?;
    let mut registry = MachineRegistry::load(config_dir)?;
    if let Err(error) = registry.insert(machine.clone()) {
        let _ = tokens.remove(&machine.host_id);
        return Err(error).context("the machine is paired but could not be saved");
    }
    // Pairing is a successful connection; `list` shows when it happened.
    registry.mark_connected(&machine.host_id, chrono::Utc::now())?;
    println!(
        "Paired with {} ({}). Saved as \"{}\"; `rustcode machines list` shows it.",
        welcome.machine_name(),
        machine.host_id,
        machine.name
    );
    Ok(())
}

/// Read the eight-digit code the machine shows, for a person at a terminal.
///
/// The read is bounded, and the code must be exactly the digits the machine
/// reported: a pasted line, a shorter code or a stray key is refused here
/// rather than spent as a failed pairing attempt on the machine.
fn read_code(digits: u32) -> Result<String> {
    print!("Enter the {digits}-digit code the machine shows: ");
    let _ = std::io::stdout().flush();
    let mut reader = std::io::stdin().lock().take(MAX_CODE_INPUT_BYTES);
    read_code_from(&mut reader, digits)
}

fn read_code_from(reader: &mut impl BufRead, digits: u32) -> Result<String> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("could not read the code from standard input")?;
    parse_code(&line, digits)
}

/// The digits of `raw`, if they are exactly the ones the machine shows.
/// Spaces and dashes are separators; anything else is not a code.
fn parse_code(raw: &str, digits: u32) -> Result<String> {
    let code = normalize_code(raw);
    ensure!(
        code.len() == digits as usize,
        "a pairing code is exactly {digits} digits; got {} (spaces and dashes are ignored, \
         letters and other characters are not)",
        code.len()
    );
    Ok(code)
}

/// The one machine Bonjour found, or an error naming what to choose from.
async fn pick_discovered(duration: Duration) -> Result<String> {
    let found = browse_async(duration)
        .await
        .context("could not browse the network")?;
    match found.as_slice() {
        [] => bail!(
            "no machine on this network accepts a desktop client; check that RustCode runs on it \
             and that both are on the same network, or pass an address"
        ),
        [one] => one
            .address()
            .with_context(|| format!("{} announced no usable address", one.name)),
        several => bail!(
            "{} machines answered; pass the address of the one to pair with:\n{}",
            several.len(),
            discovered_table(several)
        ),
    }
}

async fn discover(duration: Duration, json: bool) -> Result<()> {
    let found = browse_async(duration)
        .await
        .context("could not browse the network")?;
    if json {
        let machines: Vec<serde_json::Value> = found
            .iter()
            .map(|machine| {
                serde_json::json!({
                    "name": machine.name,
                    "address": machine.address(),
                    "addresses": machine.addresses.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "port": machine.port,
                    "gateway_id": machine.gateway_id,
                    "platform": platform(),
                    "caps": machine.caps,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&machines).unwrap_or_default()
        );
        return Ok(());
    }
    match found.is_empty() {
        true => println!(
            "No machines found. RustCode must be running with remote access on, on the same \
             network; an overlay network (Tailscale, NetBird) is not announced by Bonjour — add \
             that address with `rustcode machines add <address>`."
        ),
        false => println!("{}", discovered_table(&found)),
    }
    Ok(())
}

fn discovered_table(found: &[DiscoveredMachine]) -> String {
    let mut table = vec![vec![
        "NAME".to_owned(),
        "ADDRESS".to_owned(),
        "MACHINE ID".to_owned(),
    ]];
    for machine in found {
        table.push(vec![
            machine.name.clone(),
            machine.address().unwrap_or_else(|| "—".to_owned()),
            machine.gateway_id.clone().unwrap_or_else(|| "—".to_owned()),
        ]);
    }
    columns(&table)
}

fn remove(config_dir: &std::path::Path, selector: &str) -> Result<()> {
    let mut registry = MachineRegistry::load(config_dir)?;
    let machine = registry.select(selector)?.clone();
    registry.remove(&machine.host_id)?;
    match TokenStore::open(config_dir).and_then(|tokens| tokens.remove(&machine.host_id)) {
        Ok(()) => {}
        // The record is gone, which is what the user asked for; the token is
        // unusable without it, so a failure here is a warning, not an error.
        Err(error) => eprintln!(
            "warning: {} was removed but its token could not be deleted: {error:#}",
            machine.name
        ),
    }
    println!(
        "Removed {}. The machine still lists this computer until you revoke it there.",
        machine.name
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_pads_columns_and_leaves_no_trailing_spaces() {
        let table = columns(&[
            vec!["NAME".into(), "STATE".into()],
            vec!["Studio".into(), "connected".into()],
            vec!["Lars's Windows PC".into(), "offline".into()],
        ]);
        assert_eq!(
            table,
            "NAME               STATE\n\
             Studio             connected\n\
             Lars's Windows PC  offline"
        );
        for line in table.lines() {
            assert_eq!(line.trim_end(), line, "{line:?}");
        }
    }

    /// The code is exactly what the machine shows: separators are dropped,
    /// anything else is refused before it can cost a pairing attempt.
    #[test]
    fn a_code_must_be_exactly_the_digits_the_machine_shows() {
        assert_eq!(parse_code("4821-9034", 8).unwrap(), "48219034");
        assert_eq!(parse_code(" 4821 9034 \n", 8).unwrap(), "48219034");
        assert_eq!(parse_code("48219034", 8).unwrap(), "48219034");
        for raw in ["", "4821", "482190341", "4821-903x"] {
            let error = parse_code(raw, 8).unwrap_err().to_string();
            assert!(error.contains("exactly 8 digits"), "{raw:?} gave {error}");
        }
    }

    /// A pasted line longer than the bound is cut at it, so it can never be
    /// read as the right code.
    #[test]
    fn a_long_input_is_bounded_and_refused() {
        let long = "4".repeat(4096);
        let mut reader = std::io::Cursor::new(long).take(MAX_CODE_INPUT_BYTES);
        let error = read_code_from(&mut reader, 8).unwrap_err().to_string();
        assert!(error.contains("exactly 8 digits"), "{error}");
        // Eight digits behind a wall of spaces are cut away with the rest.
        let padded = format!("{}48219034\n", " ".repeat(200));
        let mut reader = std::io::Cursor::new(padded).take(MAX_CODE_INPUT_BYTES);
        assert!(read_code_from(&mut reader, 8).is_err());
        // A plain line still reads.
        let mut reader = std::io::Cursor::new("4821 9034\n").take(MAX_CODE_INPUT_BYTES);
        assert_eq!(read_code_from(&mut reader, 8).unwrap(), "48219034");
    }

    #[test]
    fn the_state_labels_are_what_a_person_reads() {
        assert_eq!(State::Connected.label(), "connected");
        assert_eq!(State::Offline("timed out".into()).label(), "offline");
        assert_eq!(
            State::NeedsPairing("unknown device".into()).label(),
            "pairing expired"
        );
        assert_eq!(State::NoToken.label(), "not paired");
        assert_eq!(State::NoToken.detail(), None);
        assert_eq!(
            State::NeedsPairing("unknown device".into()).detail(),
            Some("unknown device")
        );
    }

    /// `machines list` must say why the other kind of route is not used in
    /// both directions: a LAN connection with a blocked NetBird address, and
    /// an away-from-home overlay connection whose LAN route failed.
    #[test]
    fn a_hint_merges_the_races_failures_with_the_overlays_probe() {
        let lan = Route::with_kind(rustcode_client::RouteKind::Lan, "192.168.1.20:17879");
        let mut netbird = Route::with_kind(rustcode_client::RouteKind::Vpn, "100.90.16.237:17879");
        netbird.provider = Some(rustcode_client::OverlayProvider::Netbird);
        let failure = |route: Route, reason: &str| RouteFailure {
            route,
            reason: reason.into(),
            refusal: None,
        };
        let mut machine = Machine::new(
            "gw-a",
            "Studio",
            "macos",
            "device-1",
            lan.clone(),
            chrono::Utc::now(),
        );
        machine.routes.push(netbird.clone());
        // Both machines on NetBird: this computer has an address in the space
        // the machine's overlay route is in.
        let here: Vec<std::net::IpAddr> = ["192.168.1.20", "100.92.13.44"]
            .iter()
            .map(|address| address.parse().unwrap())
            .collect();
        let home = hint(
            &machine,
            &lan,
            Vec::new(),
            vec![failure(netbird.clone(), "connection timed out")],
            &here,
        )
        .expect("the LAN works and NetBird does not");
        assert!(home.contains("NetBird"), "{home}");
        assert!(home.contains("100.90.16.237:17879"), "{home}");
        let away = hint(
            &machine,
            &netbird,
            vec![failure(lan.clone(), "no route to host")],
            Vec::new(),
            &here,
        )
        .expect("the overlay works and the LAN does not");
        assert!(away.contains("not on the LAN"), "{away}");
        assert!(away.contains("192.168.1.20:17879"), "{away}");
    }

    /// Under a machine, the hint about another kind of route is printed before
    /// its threads, never between them (#3421).
    #[test]
    fn a_machines_hint_is_printed_before_its_threads() {
        let row = Row {
            machine: Machine::new(
                "gw-a",
                "Studio",
                "macos",
                "device-1",
                Route::new("192.168.1.20:17879"),
                chrono::Utc::now(),
            ),
            route: "192.168.1.20:17879".into(),
            state: State::Connected,
            hint: Some("Not reachable over NetBird (100.90.16.237:17879).".into()),
            threads: Some(vec!["Fix the login form".into(), "Bump the version".into()]),
        };
        assert_eq!(
            machine_lines(&row),
            [
                "Not reachable over NetBird (100.90.16.237:17879).",
                "Fix the login form",
                "Bump the version",
            ]
        );
        // An offline machine says why, and has no threads to print.
        let offline = Row {
            state: State::Offline("connection refused".into()),
            hint: None,
            threads: None,
            ..row
        };
        assert_eq!(machine_lines(&offline), ["connection refused"]);
    }
}
