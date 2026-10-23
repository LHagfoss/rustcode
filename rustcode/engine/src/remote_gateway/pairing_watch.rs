//! Following one pairing challenge after it is shown: the countdown, the
//! wrong codes left, a lockout, and how it ended.
//!
//! The pairing details are printed once; this turns the gateway's status into
//! one line that `/remote` and `rustcode remote pair` refresh every second,
//! until the challenge they showed is paired, used up, expired or replaced.
//! It reads only [`GatewayStatus`], so no secret is involved.

use super::control::{GatewayStatus, OfferDetails};
use super::pairing::{ATTEMPTS_PER_CHALLENGE, ChallengeEnd};
use std::time::Duration;

/// How often the line is refreshed.
pub const REFRESH: Duration = Duration::from_secs(1);

/// Where the watched challenge stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingProgress {
    /// Still redeemable, unless pairing is locked on this host.
    Open {
        expires_in_secs: u64,
        attempts_left: u8,
        locked_for_secs: Option<u64>,
    },
    /// A device paired with it; `None` when the device could not be stored.
    Paired {
        device: Option<String>,
    },
    /// Every wrong code it tolerates was used.
    UsedUp,
    Expired,
    /// A newer challenge (another `/remote pair`) replaced it.
    Replaced,
    /// The gateway restarted, so the challenge is gone.
    Restarted,
    /// The gateway is not running any more.
    Stopped,
    /// The gateway does not number its challenges: it cannot be followed.
    Unknown,
}

impl PairingProgress {
    /// Nothing will change any more; stop refreshing.
    pub fn is_final(&self) -> bool {
        !matches!(self, Self::Open { .. })
    }
}

/// The challenge being followed, and what the gateway knew before it.
#[derive(Debug, Clone)]
pub struct PairingWatch {
    challenge: Option<u64>,
    instance_id: Option<String>,
    known_devices: Vec<String>,
}

impl PairingWatch {
    /// `before` is the gateway's status from just before the challenge was
    /// issued, so a device that pairs at once is still recognised as new.
    pub fn new(offer: &OfferDetails, before: Option<&GatewayStatus>) -> Self {
        Self {
            challenge: offer.challenge,
            instance_id: before.map(|status| status.instance_id.clone()),
            known_devices: before
                .map(|status| status.devices.iter().map(|d| d.id.clone()).collect())
                .unwrap_or_default(),
        }
    }

    pub fn observe(&self, status: Option<&GatewayStatus>) -> PairingProgress {
        let Some(status) = status else {
            return PairingProgress::Stopped;
        };
        if self
            .instance_id
            .as_ref()
            .is_some_and(|instance| *instance != status.instance_id)
        {
            return PairingProgress::Restarted;
        }
        let (Some(ours), Some(pairing)) = (self.challenge, status.pairing.as_ref()) else {
            return PairingProgress::Unknown;
        };
        if pairing.challenge == Some(ours) {
            return PairingProgress::Open {
                expires_in_secs: pairing.expires_in_secs.unwrap_or_default(),
                attempts_left: pairing.attempts_left.unwrap_or_default(),
                locked_for_secs: pairing.locked_for_secs,
            };
        }
        match pairing.ended {
            Some(ended) if ended.challenge == ours => match ended.end {
                ChallengeEnd::Paired => PairingProgress::Paired {
                    device: status
                        .devices
                        .iter()
                        .find(|device| !self.known_devices.contains(&device.id))
                        .map(|device| device.name.clone()),
                },
                ChallengeEnd::UsedUp => PairingProgress::UsedUp,
            },
            _ if pairing.last_challenge.is_some_and(|last| last > ours) => {
                PairingProgress::Replaced
            }
            _ => PairingProgress::Expired,
        }
    }
}

/// One line for `progress`; `again` names the command that issues a new
/// challenge. `None` when there is nothing to add to the static details.
pub fn format_progress(progress: &PairingProgress, again: &str) -> Option<String> {
    Some(match progress {
        PairingProgress::Open {
            expires_in_secs,
            locked_for_secs: Some(locked),
            ..
        } => format!(
            "Pairing is locked for {} after too many wrong codes on this host: even the right code is refused until then. This code expires in {}.",
            clock(*locked),
            clock(*expires_in_secs)
        ),
        PairingProgress::Open {
            expires_in_secs,
            attempts_left,
            locked_for_secs: None,
        } => format!(
            "Waiting for a device: expires in {}, {attempts_left} wrong code{} left.",
            clock(*expires_in_secs),
            if *attempts_left == 1 { "" } else { "s" }
        ),
        PairingProgress::Paired { device: Some(name) } => {
            format!("Paired \"{name}\". This code is used; run {again} to add another device.")
        }
        PairingProgress::Paired { device: None } => format!(
            "This code was used, but no device was added (see `rustcode remote status`). Run {again} for a new code."
        ),
        PairingProgress::UsedUp => format!(
            "This code is used up: {ATTEMPTS_PER_CHALLENGE} wrong codes were entered. Run {again} for a new code."
        ),
        PairingProgress::Expired => format!("This code expired. Run {again} for a new code."),
        PairingProgress::Replaced => {
            "A newer pairing code replaced this one; use the code shown with it.".to_owned()
        }
        PairingProgress::Restarted => {
            format!(
                "The remote gateway restarted, so this code no longer works. Run {again} for a new code."
            )
        }
        PairingProgress::Stopped => {
            "The remote gateway stopped, so this code no longer works.".to_owned()
        }
        PairingProgress::Unknown => return None,
    })
}

/// `m:ss`, or plain seconds under a minute.
fn clock(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} s")
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote_gateway::address::AdvertisedAddress;
    use crate::remote_gateway::control::{DeviceSummary, EndedChallenge, PairingStatus};
    use crate::remote_gateway::handshake::Secret;
    use crate::remote_gateway::pairing::PairingOffer;

    fn offer(challenge: Option<u64>) -> OfferDetails {
        let mut offer = OfferDetails::new(
            &AdvertisedAddress::new("192.168.1.20", 17879).unwrap(),
            "gw",
            None,
            PairingOffer {
                credential: Secret::new("credential"),
                code: Secret::new("1234-5678"),
                lifetime: Duration::from_secs(120),
            },
        );
        offer.challenge = challenge;
        offer
    }

    fn status(pairing: PairingStatus, devices: &[(&str, &str)]) -> GatewayStatus {
        let mut status: GatewayStatus = serde_json::from_value(serde_json::json!({
            "pid": 7, "process_start_time": 1, "instance_id": "i", "gateway_id": "g",
            "uptime_seconds": 3, "listen_address": "192.168.1.20:17879",
            "advertised_address": "192.168.1.20:17879", "paired_devices": 0,
            "connections": [], "pairing_open": false,
        }))
        .unwrap();
        status.pairing = Some(pairing);
        status.devices = devices
            .iter()
            .map(|(id, name)| DeviceSummary {
                id: (*id).into(),
                name: (*name).into(),
                last_seen_at: None,
            })
            .collect();
        status
    }

    fn open(challenge: u64, expires: u64, attempts: u8) -> PairingStatus {
        PairingStatus {
            expires_in_secs: Some(expires),
            attempts_left: Some(attempts),
            challenge: Some(challenge),
            last_challenge: Some(challenge),
            ..PairingStatus::default()
        }
    }

    #[test]
    fn counts_down_the_open_challenge_and_shows_a_lockout() {
        let before = status(PairingStatus::default(), &[("aa", "phone")]);
        let watch = PairingWatch::new(&offer(Some(3)), Some(&before));
        let progress = watch.observe(Some(&status(open(3, 87, 4), &[])));
        assert_eq!(
            progress,
            PairingProgress::Open {
                expires_in_secs: 87,
                attempts_left: 4,
                locked_for_secs: None
            }
        );
        assert!(!progress.is_final());
        assert_eq!(
            format_progress(&progress, "/remote pair").unwrap(),
            "Waiting for a device: expires in 1:27, 4 wrong codes left."
        );

        let mut locked = open(3, 40, 2);
        locked.locked_for_secs = Some(250);
        let line =
            format_progress(&watch.observe(Some(&status(locked, &[]))), "/remote pair").unwrap();
        assert!(line.starts_with("Pairing is locked for 4:10"), "{line}");
        assert!(line.contains("expires in 40 s"), "{line}");
    }

    #[test]
    fn says_how_the_challenge_ended() {
        let before = status(PairingStatus::default(), &[("aa", "phone")]);
        let watch = PairingWatch::new(&offer(Some(3)), Some(&before));
        let ended = |end| PairingStatus {
            last_challenge: Some(3),
            ended: Some(EndedChallenge { challenge: 3, end }),
            ..PairingStatus::default()
        };

        let paired = watch.observe(Some(&status(
            ended(ChallengeEnd::Paired),
            &[("aa", "phone"), ("bb", "Lars's iPad")],
        )));
        assert_eq!(
            paired,
            PairingProgress::Paired {
                device: Some("Lars's iPad".into())
            }
        );
        assert!(paired.is_final());
        assert!(
            format_progress(&paired, "/remote pair")
                .unwrap()
                .starts_with("Paired \"Lars's iPad\".")
        );

        let used = watch.observe(Some(&status(ended(ChallengeEnd::UsedUp), &[])));
        assert_eq!(used, PairingProgress::UsedUp);
        assert!(
            format_progress(&used, "`rustcode remote pair`")
                .unwrap()
                .contains("5 wrong codes were entered. Run `rustcode remote pair`")
        );

        let expired = PairingStatus {
            last_challenge: Some(3),
            ..PairingStatus::default()
        };
        assert_eq!(
            watch.observe(Some(&status(expired, &[]))),
            PairingProgress::Expired
        );
        assert_eq!(
            watch.observe(Some(&status(open(4, 120, 5), &[]))),
            PairingProgress::Replaced
        );
        assert_eq!(watch.observe(None), PairingProgress::Stopped);

        let mut restarted = status(open(1, 120, 5), &[]);
        restarted.instance_id = "other".into();
        assert_eq!(watch.observe(Some(&restarted)), PairingProgress::Restarted);
    }

    #[test]
    fn an_older_gateway_is_not_followed() {
        let watch = PairingWatch::new(&offer(None), None);
        let progress = watch.observe(Some(&status(open(1, 120, 5), &[])));
        assert_eq!(progress, PairingProgress::Unknown);
        assert!(progress.is_final());
        assert_eq!(format_progress(&progress, "/remote pair"), None);
    }
}
