//! The gateway's relay registration ([ADR 0013](../../../../../docs/adr/0013-machine-relay-and-nat-traversal.md)):
//! the member loop of [`crate::remote_relay::member`] with this gateway's
//! paired device keys, its connection handling for each pipe, and the relay
//! route a handshake reports while it is registered.

use super::super::address::{HostRoute, HostRouteKind, MAX_ROUTES};
use super::super::transport;
use super::{Arrival, Shared};
use crate::remote_relay::member::{self, Host};
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;

/// The `rustcode-relay://host:port` this machine is reachable at now.
#[derive(Default)]
pub(in super::super) struct RelayRoute(Mutex<Option<String>>);

impl RelayRoute {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<String>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The route, or an empty string while not registered.
    pub(in super::super) fn address(&self) -> String {
        self.lock().clone().unwrap_or_default()
    }

    /// `routes` with the relay route last, while registered.
    pub(super) fn append(&self, mut routes: Vec<HostRoute>) -> Vec<HostRoute> {
        if let Some(address) = self.lock().clone()
            && routes.len() < MAX_ROUTES
        {
            routes.push(HostRoute {
                address,
                kind: HostRouteKind::Relay,
                provider: None,
            });
        }
        routes
    }
}

struct GatewayHost(Arc<Shared>);

impl Host for GatewayHost {
    fn allowed_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .0
            .state()
            .devices
            .list()
            .iter()
            .filter_map(|record| record.public_key.clone())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    fn deliver(&self, pipe: TcpStream) {
        let Ok(peer) = pipe.peer_addr() else { return };
        if let Some(slot) = self.0.admit_socket(peer.ip()) {
            tokio::spawn(transport::serve_connection(
                Arc::clone(&self.0),
                pipe,
                slot,
                Arrival::Relay,
            ));
        }
    }

    fn set_route(&self, route: Option<String>) {
        *self.0.relay.lock() = route;
    }
}

/// Keep this gateway registered with its relay, if it has one, and run the
/// relay a hub offers its members, until the gateway shuts down.
pub(super) async fn run(shared: Arc<Shared>) {
    let config_dir = shared.lifecycle.config_directory();
    let machine_id = shared.gateway_id.clone();
    let stop = shared.shutdown.clone();
    let hub_relay = super::hub_relay::run(Arc::clone(&shared));
    let member = member::run(config_dir, machine_id, Arc::new(GatewayHost(shared)), stop);
    tokio::join!(hub_relay, member);
}
