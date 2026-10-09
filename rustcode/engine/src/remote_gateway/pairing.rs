//! Pairing challenges: one outstanding challenge with two representations.
//!
//! The QR payload carries a single-use high-entropy credential; manual
//! pairing uses the advertised address plus a short code. Both belong to the
//! same challenge: they share its lifetime and attempt budget and are
//! consumed together. Only digests are kept after the offer is handed out.
//!
//! The state is plain data behind the gateway's one state lock, so redeeming
//! is atomic: two sockets racing on one challenge cannot both win. Time is
//! passed in, which keeps expiry and rate limits testable without sleeping.

use super::address::AdvertisedAddress;
use super::handshake::{HANDSHAKE_PROTOCOL_VERSION, PairingMethod, Secret};
use base64::Engine as _;
use rand::RngExt as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a challenge can be redeemed.
pub const CHALLENGE_LIFETIME: Duration = Duration::from_secs(120);

/// Wrong secrets a single challenge tolerates before it is destroyed.
pub const ATTEMPTS_PER_CHALLENGE: u8 = 5;

/// Failed pairing attempts the whole host tolerates per window, counted
/// across every connection and every challenge.
pub const HOST_FAILURE_LIMIT: usize = 10;
pub const HOST_FAILURE_WINDOW: Duration = Duration::from_secs(300);

const CODE_DIGITS: usize = 8;

/// A freshly issued challenge. The only place the secrets exist in the clear.
#[derive(Debug, Clone)]
pub struct PairingOffer {
    /// 256 random bits, base64url. Goes into the QR payload.
    pub credential: Secret,
    /// Eight digits, formatted `1234-5678`, typed next to the address.
    pub code: Secret,
    pub lifetime: Duration,
}

/// What the QR code encodes. Compact JSON; see [`qr_payload`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QrPayload {
    pub protocol_version: u32,
    /// `host:port` of the WebSocket listener, never an unspecified address.
    pub address: String,
    pub gateway_id: String,
    pub credential: Secret,
    /// The host's own name, for display; cut to keep the code scannable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
}

/// Longest host name the QR payload carries.
pub const QR_HOST_NAME_CHARS: usize = 24;

/// The exact string a pairing QR code encodes; [`super::qr`] draws it.
pub fn qr_payload(
    address: &AdvertisedAddress,
    gateway_id: &str,
    credential: &Secret,
    host_name: Option<&str>,
) -> String {
    serde_json::to_string(&QrPayload {
        protocol_version: HANDSHAKE_PROTOCOL_VERSION,
        address: address.to_string(),
        gateway_id: gateway_id.to_string(),
        credential: credential.clone(),
        host_name: host_name.map(|name| name.chars().take(QR_HOST_NAME_CHARS).collect()),
    })
    .expect("QR payload always serializes")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingError {
    /// Wrong, expired, exhausted, already used, or no challenge at all.
    Failed,
    /// The host-wide failure budget is spent. Nothing was compared.
    RateLimited { retry_after: Duration },
}

struct Challenge {
    credential_digest: [u8; 32],
    code_digest: [u8; 32],
    expires_at: Instant,
    attempts_left: u8,
}

#[derive(Default)]
pub struct PairingState {
    challenge: Option<Challenge>,
    /// Times of recent failed attempts, oldest first; at most `HOST_FAILURE_LIMIT`.
    failures: VecDeque<Instant>,
}

impl PairingState {
    /// Issue a challenge, replacing any outstanding one. Only the local owner
    /// can reach this (through the private control socket), so a network peer
    /// cannot refill its own attempt budget.
    pub fn issue(&mut self, now: Instant) -> PairingOffer {
        let mut bytes = [0u8; 32];
        rand::rng().fill(&mut bytes);
        let credential = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        let number = rand::rng().random_range(0..10u32.pow(CODE_DIGITS as u32));
        let digits = format!("{number:0width$}", width = CODE_DIGITS);
        self.challenge = Some(Challenge {
            credential_digest: digest(credential.as_bytes()),
            code_digest: digest(digits.as_bytes()),
            expires_at: now + CHALLENGE_LIFETIME,
            attempts_left: ATTEMPTS_PER_CHALLENGE,
        });
        PairingOffer {
            credential: Secret::new(credential),
            code: Secret::new(format!("{}-{}", &digits[..4], &digits[4..])),
            lifetime: CHALLENGE_LIFETIME,
        }
    }

    /// Whether a challenge could still be redeemed at `now`.
    pub fn is_open(&self, now: Instant) -> bool {
        self.challenge
            .as_ref()
            .is_some_and(|challenge| now < challenge.expires_at && challenge.attempts_left > 0)
    }

    /// Try to redeem the outstanding challenge. Success destroys it, and with
    /// it both representations.
    pub fn redeem(
        &mut self,
        now: Instant,
        method: PairingMethod,
        presented: &str,
    ) -> Result<(), PairingError> {
        // Fail closed while rate limited: not even a correct secret is
        // compared, so a locked-out guesser learns nothing.
        if let Some(retry_after) = self.locked_for(now) {
            return Err(PairingError::RateLimited { retry_after });
        }
        if !self.is_open(now) {
            self.challenge = None;
            return Err(self.fail(now));
        }
        let challenge = self.challenge.as_mut().expect("challenge is open");
        // Spend the attempt before comparing so a failure cannot be retried free.
        challenge.attempts_left -= 1;
        let matches = match method {
            PairingMethod::Credential => {
                constant_time_eq(&digest(presented.as_bytes()), &challenge.credential_digest)
            }
            PairingMethod::Code => constant_time_eq(
                &digest(normalize_code(presented).as_bytes()),
                &challenge.code_digest,
            ),
        };
        if matches {
            self.challenge = None;
            return Ok(());
        }
        if challenge.attempts_left == 0 {
            self.challenge = None;
        }
        Err(self.fail(now))
    }

    fn fail(&mut self, now: Instant) -> PairingError {
        self.prune(now);
        if self.failures.len() == HOST_FAILURE_LIMIT {
            self.failures.pop_front();
        }
        self.failures.push_back(now);
        PairingError::Failed
    }

    fn prune(&mut self, now: Instant) {
        while self
            .failures
            .front()
            .is_some_and(|failed| now.duration_since(*failed) >= HOST_FAILURE_WINDOW)
        {
            self.failures.pop_front();
        }
    }

    /// Remaining lockout, if the failure budget for the current window is spent.
    fn locked_for(&mut self, now: Instant) -> Option<Duration> {
        self.prune(now);
        if self.failures.len() < HOST_FAILURE_LIMIT {
            return None;
        }
        let oldest = *self.failures.front()?;
        Some(HOST_FAILURE_WINDOW.saturating_sub(now.duration_since(oldest)))
    }
}

/// Accept the code as displayed (`1234-5678`) or typed without the separator.
fn normalize_code(presented: &str) -> String {
    presented
        .chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .collect()
}

pub(super) fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Compare two digests without returning early on the first difference.
/// Inputs are fixed-length digests, so neither length nor a matching prefix
/// of the presented secret is observable.
pub(super) fn constant_time_eq(left: &[u8; 32], right: &[u8; 32]) -> bool {
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    std::hint::black_box(difference) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrong_code(state: &mut PairingState, now: Instant) -> Result<(), PairingError> {
        state.redeem(now, PairingMethod::Code, "not-the-code")
    }

    #[test]
    fn offer_has_a_high_entropy_credential_and_a_short_code() {
        let mut state = PairingState::default();
        let now = Instant::now();
        let first = state.issue(now);
        let second = state.issue(now);
        // 32 random bytes, base64url without padding.
        assert_eq!(first.credential.expose().len(), 43);
        assert_ne!(first.credential, second.credential);
        let code = first.code.expose();
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(code.chars().filter(char::is_ascii_digit).count() == 8);
        assert_eq!(first.lifetime, Duration::from_secs(120));
    }

    #[test]
    fn either_representation_pairs_once_and_consumes_both() {
        let now = Instant::now();
        for method in [PairingMethod::Credential, PairingMethod::Code] {
            let mut state = PairingState::default();
            let offer = state.issue(now);
            let secret = match method {
                PairingMethod::Credential => offer.credential.expose(),
                PairingMethod::Code => offer.code.expose(),
            };
            assert_eq!(state.redeem(now, method, secret), Ok(()));
            assert!(!state.is_open(now));
            // Replay of the used representation, and use of the other one.
            assert_eq!(
                state.redeem(now, PairingMethod::Credential, offer.credential.expose()),
                Err(PairingError::Failed)
            );
            assert_eq!(
                state.redeem(now, PairingMethod::Code, offer.code.expose()),
                Err(PairingError::Failed)
            );
        }
    }

    #[test]
    fn code_is_accepted_with_or_without_its_separator() {
        let now = Instant::now();
        let mut state = PairingState::default();
        let offer = state.issue(now);
        let bare = offer.code.expose().replace('-', "");
        assert_eq!(state.redeem(now, PairingMethod::Code, &bare), Ok(()));
    }

    #[test]
    fn a_secret_is_only_valid_for_its_own_method() {
        let now = Instant::now();
        let mut state = PairingState::default();
        let offer = state.issue(now);
        assert_eq!(
            state.redeem(now, PairingMethod::Credential, offer.code.expose()),
            Err(PairingError::Failed)
        );
        assert_eq!(
            state.redeem(now, PairingMethod::Code, offer.credential.expose()),
            Err(PairingError::Failed)
        );
    }

    #[test]
    fn challenge_expires_after_two_minutes() {
        let now = Instant::now();
        let mut state = PairingState::default();
        let offer = state.issue(now);
        let just_before = now + CHALLENGE_LIFETIME - Duration::from_millis(1);
        assert!(state.is_open(just_before));
        let at_expiry = now + CHALLENGE_LIFETIME;
        assert!(!state.is_open(at_expiry));
        assert_eq!(
            state.redeem(
                at_expiry,
                PairingMethod::Credential,
                offer.credential.expose()
            ),
            Err(PairingError::Failed)
        );
        // Expiry destroyed it; a rewound clock cannot bring it back.
        assert!(!state.is_open(now));
    }

    #[test]
    fn five_wrong_attempts_destroy_the_challenge() {
        let now = Instant::now();
        let mut state = PairingState::default();
        let offer = state.issue(now);
        for _ in 0..ATTEMPTS_PER_CHALLENGE {
            assert!(state.is_open(now));
            assert_eq!(wrong_code(&mut state, now), Err(PairingError::Failed));
        }
        assert!(!state.is_open(now));
        // The right secret, on either representation, is now worthless.
        assert_eq!(
            state.redeem(now, PairingMethod::Code, offer.code.expose()),
            Err(PairingError::Failed)
        );
        assert_eq!(
            state.redeem(now, PairingMethod::Credential, offer.credential.expose()),
            Err(PairingError::Failed)
        );
    }

    #[test]
    fn attempts_are_shared_between_both_representations() {
        let now = Instant::now();
        let mut state = PairingState::default();
        let offer = state.issue(now);
        for attempt in 0..ATTEMPTS_PER_CHALLENGE {
            let method = if attempt % 2 == 0 {
                PairingMethod::Code
            } else {
                PairingMethod::Credential
            };
            assert_eq!(
                state.redeem(now, method, "wrong"),
                Err(PairingError::Failed)
            );
        }
        assert_eq!(
            state.redeem(now, PairingMethod::Credential, offer.credential.expose()),
            Err(PairingError::Failed)
        );
    }

    #[test]
    fn host_wide_limit_survives_new_challenges_and_blocks_correct_secrets() {
        let start = Instant::now();
        let mut state = PairingState::default();
        // Two fresh challenges, each burned by five wrong guesses.
        for _ in 0..2 {
            state.issue(start);
            for _ in 0..ATTEMPTS_PER_CHALLENGE {
                assert_eq!(wrong_code(&mut state, start), Err(PairingError::Failed));
            }
        }
        // A third challenge is issued, but the host is locked out: even the
        // correct code is refused, and refusing does not consume the challenge.
        let offer = state.issue(start);
        let locked = state.redeem(start, PairingMethod::Code, offer.code.expose());
        assert_eq!(
            locked,
            Err(PairingError::RateLimited {
                retry_after: HOST_FAILURE_WINDOW
            })
        );
        assert!(state.is_open(start));
        // Hammering while locked out neither extends the lockout nor burns attempts.
        let later = start + Duration::from_secs(60);
        for _ in 0..50 {
            assert_eq!(
                wrong_code(&mut state, later),
                Err(PairingError::RateLimited {
                    retry_after: HOST_FAILURE_WINDOW - Duration::from_secs(60)
                })
            );
        }
        // Once the window has passed, a fresh challenge pairs normally.
        let after = start + HOST_FAILURE_WINDOW;
        let offer = state.issue(after);
        assert_eq!(
            state.redeem(after, PairingMethod::Code, offer.code.expose()),
            Ok(())
        );
    }

    #[test]
    fn attempts_without_a_challenge_count_against_the_host() {
        let now = Instant::now();
        let mut state = PairingState::default();
        for _ in 0..HOST_FAILURE_LIMIT {
            assert_eq!(wrong_code(&mut state, now), Err(PairingError::Failed));
        }
        assert!(matches!(
            wrong_code(&mut state, now),
            Err(PairingError::RateLimited { .. })
        ));
        assert_eq!(state.failures.len(), HOST_FAILURE_LIMIT);
    }

    #[test]
    fn qr_payload_carries_version_address_identity_and_credential() {
        let address = AdvertisedAddress::new("192.168.1.20", 17879).unwrap();
        let payload = qr_payload(&address, "gateway-1", &Secret::new("cred"), None);
        assert_eq!(
            payload,
            r#"{"protocol_version":1,"address":"192.168.1.20:17879","gateway_id":"gateway-1","credential":"cred"}"#
        );
        let decoded: QrPayload = serde_json::from_str(&payload).unwrap();
        assert_eq!(decoded.credential.expose(), "cred");
    }

    #[test]
    fn offer_debug_output_hides_both_secrets() {
        let mut state = PairingState::default();
        let offer = state.issue(Instant::now());
        let debug = format!("{offer:?}");
        assert!(!debug.contains(offer.credential.expose()));
        assert!(!debug.contains(offer.code.expose()));
    }

    #[test]
    fn digests_compare_in_full() {
        let a = digest(b"a");
        let mut b = a;
        assert!(constant_time_eq(&a, &b));
        b[31] ^= 1;
        assert!(!constant_time_eq(&a, &b));
    }
}
