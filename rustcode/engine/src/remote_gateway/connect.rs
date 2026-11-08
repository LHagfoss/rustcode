//! Connection requests: a device on the network asks to pair, and the host's
//! owner accepts or denies it on the host.
//!
//! A request carries no secret. It only puts a prompt in front of the owner;
//! accepting it issues a pairing challenge bound to the request
//! ([`super::pairing::PairingState::issue_bound`]) whose code the owner reads
//! off the host and types on the device. So a request never grants access by
//! itself, and the code still travels through the person, not the network.
//!
//! The state is plain data behind the gateway's one state lock, with time
//! passed in. Each open request has a [`tokio::sync::watch`] channel to the
//! socket that made it, which is how an answer given on the host reaches it.

use super::devices::{DeviceProfile, sanitize_name};
use rand::RngExt as _;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// How long a request waits for the owner's answer.
pub const REQUEST_LIFETIME: Duration = Duration::from_secs(120);

/// Requests open at once, across every device.
pub const MAX_OPEN_REQUESTS: usize = 4;

/// How long a network address waits after a denial before it may ask again.
pub const DENY_COOLDOWN: Duration = Duration::from_secs(30);

/// Ended requests the status keeps, newest last.
const ENDED_KEPT: usize = 8;

/// What the socket that made a request is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Still waiting for the owner.
    Pending,
    /// A challenge bound to the request is open for this long.
    Approved {
        expires_in: Duration,
    },
    Denied,
    /// The request, or the challenge issued for it, ran out or was replaced.
    Expired,
}

/// Where an open request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    /// Waiting for the owner.
    Pending,
    /// Accepted: its code is on the host and the device may present it.
    Approved,
}

/// How a request ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestEnd {
    /// The device presented the code and is paired.
    Paired,
    Denied,
    /// Nobody answered in time, or the code was not presented in time.
    Expired,
    /// Another accepted request or a fresh pairing code took its place.
    Replaced,
    /// The device presented a wrong code, or pairing failed on the host.
    Failed,
    /// The device closed the connection.
    Withdrawn,
}

pub struct Request {
    pub id: String,
    pub device_name: String,
    /// What the device said it is; recorded with the device if it pairs.
    pub profile: DeviceProfile,
    install_id: Option<String>,
    pub remote_addr: IpAddr,
    pub created: Instant,
    /// When the request, or once approved its challenge, runs out.
    pub expires_at: Instant,
    pub state: RequestState,
    outcome: watch::Sender<Outcome>,
}

/// A request that ended recently, for the host to say what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub id: String,
    pub device_name: String,
    pub end: RequestEnd,
    /// The device the request paired, when it did.
    pub device_id: Option<String>,
    pub at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// The host holds as many requests as it takes, or this address
    /// already has one open.
    Busy,
    /// This address was denied a moment ago.
    RateLimited { retry_after: Duration },
}

/// A newly opened request, as its socket follows it.
pub struct Opened {
    pub id: String,
    pub expires_at: Instant,
    pub outcome: watch::Receiver<Outcome>,
}

#[derive(Default)]
pub struct ConnectRequests {
    open: Vec<Request>,
    cooldowns: HashMap<IpAddr, Instant>,
    ended: VecDeque<Ended>,
}

impl ConnectRequests {
    /// Open a request from `remote_addr`. The caller has already ended any
    /// request this one replaces ([`Self::replaceable`]).
    pub fn open(
        &mut self,
        now: Instant,
        lifetime: Duration,
        remote_addr: IpAddr,
        device_name: &str,
        profile: DeviceProfile,
        install_id: Option<&str>,
    ) -> Result<Opened, OpenError> {
        let remote_addr = remote_addr.to_canonical();
        self.cooldowns.retain(|_, until| *until > now);
        if let Some(until) = self.cooldowns.get(&remote_addr) {
            return Err(OpenError::RateLimited {
                retry_after: until.duration_since(now),
            });
        }
        if self.open.len() >= MAX_OPEN_REQUESTS
            || self
                .open
                .iter()
                .any(|request| request.remote_addr == remote_addr)
        {
            return Err(OpenError::Busy);
        }
        let mut bytes = [0u8; 8];
        rand::rng().fill(&mut bytes);
        let id = hex::encode(bytes);
        let (sender, receiver) = watch::channel(Outcome::Pending);
        let expires_at = now + lifetime;
        self.open.push(Request {
            id: id.clone(),
            device_name: sanitize_name(device_name),
            profile,
            install_id: install_id.map(str::to_owned),
            remote_addr,
            created: now,
            expires_at,
            state: RequestState::Pending,
            outcome: sender,
        });
        Ok(Opened {
            id,
            expires_at,
            outcome: receiver,
        })
    }

    /// The open request a new one from the same address and app install
    /// would replace: the app reconnected, and its old socket is likely dead.
    pub fn replaceable(&self, remote_addr: IpAddr, install_id: Option<&str>) -> Option<String> {
        let install_id = install_id?;
        let remote_addr = remote_addr.to_canonical();
        self.open
            .iter()
            .find(|request| {
                request.remote_addr == remote_addr
                    && request.install_id.as_deref() == Some(install_id)
            })
            .map(|request| request.id.clone())
    }

    pub fn get(&self, id: &str) -> Option<&Request> {
        self.open.iter().find(|request| request.id == id)
    }

    /// Open requests, oldest first.
    pub fn list(&self) -> &[Request] {
        &self.open
    }

    /// Recently ended requests, oldest first.
    pub fn ended(&self) -> impl Iterator<Item = &Ended> {
        self.ended.iter()
    }

    /// The approved request, if one is open. There is at most one, because
    /// there is at most one challenge.
    pub fn approved(&self) -> Option<&Request> {
        self.open
            .iter()
            .find(|request| request.state == RequestState::Approved)
    }

    /// Mark a pending request approved with a challenge open until
    /// `expires_at`, and tell its socket. `false` when it is not pending.
    pub fn approve(&mut self, now: Instant, id: &str, expires_at: Instant) -> bool {
        let Some(request) = self
            .open
            .iter_mut()
            .find(|request| request.id == id && request.state == RequestState::Pending)
        else {
            return false;
        };
        request.state = RequestState::Approved;
        request.expires_at = expires_at;
        let _ = request.outcome.send(Outcome::Approved {
            expires_in: expires_at.saturating_duration_since(now),
        });
        true
    }

    /// End an open request, tell its socket when there is something to tell,
    /// and remember how it ended. A denial also starts its address's
    /// cooldown. `None` when no such request is open.
    pub fn finish(
        &mut self,
        now: Instant,
        id: &str,
        end: RequestEnd,
        device_id: Option<String>,
    ) -> Option<Request> {
        let index = self.open.iter().position(|request| request.id == id)?;
        let request = self.open.remove(index);
        match end {
            RequestEnd::Denied => {
                let _ = request.outcome.send(Outcome::Denied);
                self.cooldowns
                    .insert(request.remote_addr, now + DENY_COOLDOWN);
            }
            RequestEnd::Expired | RequestEnd::Replaced => {
                let _ = request.outcome.send(Outcome::Expired);
            }
            RequestEnd::Paired | RequestEnd::Failed | RequestEnd::Withdrawn => {}
        }
        if self.ended.len() == ENDED_KEPT {
            self.ended.pop_front();
        }
        self.ended.push_back(Ended {
            id: request.id.clone(),
            device_name: request.device_name.clone(),
            end,
            device_id,
            at: now,
        });
        Some(request)
    }

    /// Requests whose time ran out by `now`. Each one's socket ends it on
    /// its own deadline; this is for a status read in between.
    pub fn expired(&self, now: Instant) -> Vec<String> {
        self.open
            .iter()
            .filter(|request| request.expires_at <= now)
            .map(|request| request.id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ios() -> DeviceProfile {
        DeviceProfile::from_wire(None, Some("ios"), None)
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, last))
    }

    #[test]
    fn a_request_is_bounded_and_says_who_asked() {
        let mut requests = ConnectRequests::default();
        let now = Instant::now();
        let opened = requests
            .open(
                now,
                REQUEST_LIFETIME,
                ip(2),
                "Lars's\u{1b}[31m iPhone",
                DeviceProfile::from_wire(Some("desktop"), Some("iOS 26!"), None),
                None,
            )
            .unwrap();
        assert_eq!(opened.id.len(), 16);
        assert_eq!(opened.expires_at, now + REQUEST_LIFETIME);
        assert_eq!(*opened.outcome.borrow(), Outcome::Pending);
        let request = requests.get(&opened.id).unwrap();
        assert_eq!(request.device_name, "Lars's[31m iPhone");
        assert_eq!(request.profile.platform, "ios26");
        assert_eq!(
            request.profile.kind,
            super::super::devices::DeviceKind::Desktop
        );
        assert_eq!(request.state, RequestState::Pending);
    }

    #[test]
    fn the_host_and_each_address_hold_a_bounded_number() {
        let mut requests = ConnectRequests::default();
        let now = Instant::now();
        requests
            .open(now, REQUEST_LIFETIME, ip(1), "a", ios(), None)
            .unwrap();
        assert_eq!(
            requests
                .open(now, REQUEST_LIFETIME, ip(1), "b", ios(), None)
                .err(),
            Some(OpenError::Busy)
        );
        for last in 2..=MAX_OPEN_REQUESTS as u8 {
            requests
                .open(now, REQUEST_LIFETIME, ip(last), "a", ios(), None)
                .unwrap();
        }
        assert_eq!(
            requests
                .open(now, REQUEST_LIFETIME, ip(99), "a", ios(), None)
                .err(),
            Some(OpenError::Busy)
        );
    }

    #[test]
    fn a_denial_tells_the_socket_and_cools_the_address_down() {
        let mut requests = ConnectRequests::default();
        let now = Instant::now();
        let opened = requests
            .open(now, REQUEST_LIFETIME, ip(3), "phone", ios(), None)
            .unwrap();
        requests.finish(now, &opened.id, RequestEnd::Denied, None);
        assert_eq!(*opened.outcome.borrow(), Outcome::Denied);
        assert!(requests.get(&opened.id).is_none());
        let later = now + Duration::from_secs(10);
        assert_eq!(
            requests
                .open(later, REQUEST_LIFETIME, ip(3), "phone", ios(), None)
                .err(),
            Some(OpenError::RateLimited {
                retry_after: Duration::from_secs(20)
            })
        );
        // Other addresses are not affected, and the cooldown ends.
        requests
            .open(later, REQUEST_LIFETIME, ip(4), "phone", ios(), None)
            .unwrap();
        requests
            .open(
                now + DENY_COOLDOWN,
                REQUEST_LIFETIME,
                ip(3),
                "phone",
                ios(),
                None,
            )
            .unwrap();
        let ended: Vec<_> = requests.ended().map(|ended| ended.end).collect();
        assert_eq!(ended, [RequestEnd::Denied]);
    }

    #[test]
    fn approving_tells_the_socket_how_long_the_code_lasts() {
        let mut requests = ConnectRequests::default();
        let now = Instant::now();
        let opened = requests
            .open(now, REQUEST_LIFETIME, ip(5), "phone", ios(), None)
            .unwrap();
        assert!(requests.approved().is_none());
        assert!(requests.approve(now, &opened.id, now + Duration::from_secs(120)));
        assert_eq!(
            *opened.outcome.borrow(),
            Outcome::Approved {
                expires_in: Duration::from_secs(120)
            }
        );
        assert_eq!(requests.approved().unwrap().id, opened.id);
        // An approved request cannot be approved again.
        assert!(!requests.approve(now, &opened.id, now));
        requests.finish(now, &opened.id, RequestEnd::Replaced, None);
        assert_eq!(*opened.outcome.borrow(), Outcome::Expired);
    }

    #[test]
    fn the_same_install_from_the_same_address_replaces_its_request() {
        let mut requests = ConnectRequests::default();
        let now = Instant::now();
        let install = "install-0123456789abcdef";
        let first = requests
            .open(now, REQUEST_LIFETIME, ip(6), "phone", ios(), Some(install))
            .unwrap();
        assert_eq!(requests.replaceable(ip(6), Some(install)), Some(first.id));
        assert_eq!(requests.replaceable(ip(7), Some(install)), None);
        assert_eq!(
            requests.replaceable(ip(6), Some("another-install-0000")),
            None
        );
        assert_eq!(requests.replaceable(ip(6), None), None);
    }

    #[test]
    fn ended_requests_are_kept_briefly_and_in_order() {
        let mut requests = ConnectRequests::default();
        let now = Instant::now();
        for _ in 0..ENDED_KEPT + 2 {
            let opened = requests
                .open(now, REQUEST_LIFETIME, ip(8), "phone", ios(), None)
                .unwrap();
            requests.finish(now, &opened.id, RequestEnd::Withdrawn, None);
        }
        assert_eq!(requests.ended().count(), ENDED_KEPT);
        assert!(requests.expired(now).is_empty());
        let opened = requests
            .open(now, REQUEST_LIFETIME, ip(9), "phone", ios(), None)
            .unwrap();
        assert_eq!(requests.expired(now + REQUEST_LIFETIME), vec![opened.id]);
    }
}
