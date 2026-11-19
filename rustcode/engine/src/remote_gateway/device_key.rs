//! Key-pair authentication for paired devices (issue #3956).
//!
//! A device that holds an Ed25519 key pair gives the gateway its public key
//! once: in its `pair` frame, or, for a device paired before keys, in the next
//! `authenticate` it sends with its token. From then on it proves itself by
//! signing a challenge the gateway made for that one socket, and its token is
//! refused. A signature lifted off the wire is worth nothing on another
//! socket, because each socket gets a fresh challenge and spends it once.
//!
//! What a device signs is [`signed_message`]: a domain separator, the
//! gateway's identifier, the socket's challenge and the device's identifier,
//! each followed by a zero byte. Naming the gateway keeps a signature made for
//! one machine from being accepted by another that happened to issue the same
//! challenge; naming the device keeps it from authenticating any other record.
//!
//! Keys travel as unpadded base64url of the raw 32-byte public key, and
//! signatures as unpadded base64url of the 64-byte signature. The private key
//! never leaves the device.

use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngExt as _;

/// Separates these signatures from anything else the same key could sign.
pub const SIGNATURE_DOMAIN: &str = "rustcode-remote-auth-v1";

/// Length of a raw Ed25519 public key.
const PUBLIC_KEY_BYTES: usize = 32;

/// Length of an Ed25519 signature.
const SIGNATURE_BYTES: usize = 64;

/// Random bytes in a socket's challenge.
const CHALLENGE_BYTES: usize = 32;

/// A fresh challenge for one socket.
pub fn new_challenge() -> String {
    let mut bytes = [0u8; CHALLENGE_BYTES];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The bytes a device signs to authenticate on the socket that was given
/// `challenge` by the gateway `gateway_id`.
pub fn signed_message(gateway_id: &str, challenge: &str, device_id: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(
        SIGNATURE_DOMAIN.len() + gateway_id.len() + challenge.len() + device_id.len() + 4,
    );
    for part in [SIGNATURE_DOMAIN, gateway_id, challenge, device_id] {
        message.extend_from_slice(part.as_bytes());
        message.push(0);
    }
    message
}

/// `public_key` in the form a device stores it in, when it is one: unpadded
/// base64url of exactly 32 bytes. Anything else is not a key.
pub fn normalize_public_key(public_key: &str) -> Option<String> {
    let bytes = URL_SAFE_NO_PAD.decode(public_key.trim()).ok()?;
    (bytes.len() == PUBLIC_KEY_BYTES).then(|| URL_SAFE_NO_PAD.encode(bytes))
}

/// Whether `signature` is `public_key`'s signature of `message`. A malformed
/// key or signature is simply not valid.
pub fn verify(public_key: &str, message: &[u8], signature: &str) -> bool {
    let Ok(key) = URL_SAFE_NO_PAD.decode(public_key) else {
        return false;
    };
    let Ok(signature) = URL_SAFE_NO_PAD.decode(signature) else {
        return false;
    };
    if key.len() != PUBLIC_KEY_BYTES || signature.len() != SIGNATURE_BYTES {
        return false;
    }
    UnparsedPublicKey::new(&ED25519, key)
        .verify(message, &signature)
        .is_ok()
}

#[cfg(test)]
pub(crate) mod test_keys {
    //! A device's side of the exchange, for tests.

    use super::*;
    use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};

    pub struct TestKey(Ed25519KeyPair);

    impl TestKey {
        pub fn new() -> Self {
            Self(Ed25519KeyPair::generate().expect("generate an Ed25519 key"))
        }

        pub fn public_key(&self) -> String {
            URL_SAFE_NO_PAD.encode(self.0.public_key().as_ref())
        }

        pub fn sign(&self, gateway_id: &str, challenge: &str, device_id: &str) -> String {
            let message = signed_message(gateway_id, challenge, device_id);
            URL_SAFE_NO_PAD.encode(self.0.sign(&message).as_ref())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_keys::TestKey;
    use super::*;

    #[test]
    fn a_signature_verifies_only_for_its_gateway_challenge_and_device() {
        let key = TestKey::new();
        let signature = key.sign("gw", "c1", "dev");
        let public = key.public_key();
        assert!(verify(
            &public,
            &signed_message("gw", "c1", "dev"),
            &signature
        ));
        assert!(!verify(
            &public,
            &signed_message("gw2", "c1", "dev"),
            &signature
        ));
        assert!(!verify(
            &public,
            &signed_message("gw", "c2", "dev"),
            &signature
        ));
        assert!(!verify(
            &public,
            &signed_message("gw", "c1", "dev2"),
            &signature
        ));
        let other = TestKey::new().public_key();
        assert!(!verify(
            &other,
            &signed_message("gw", "c1", "dev"),
            &signature
        ));
    }

    #[test]
    fn the_parts_cannot_be_shifted_into_each_other() {
        assert_ne!(
            signed_message("ab", "c", "d"),
            signed_message("a", "bc", "d")
        );
    }

    #[test]
    fn malformed_keys_and_signatures_are_not_valid() {
        let key = TestKey::new();
        let message = signed_message("gw", "c", "d");
        assert!(!verify("not base64!", &message, &key.sign("gw", "c", "d")));
        assert!(!verify(&key.public_key(), &message, "c2hvcnQ"));
        assert_eq!(normalize_public_key("c2hvcnQ"), None);
        assert_eq!(
            normalize_public_key(&format!(" {} ", key.public_key())),
            Some(key.public_key())
        );
    }

    #[test]
    fn challenges_are_fresh() {
        assert_ne!(new_challenge(), new_challenge());
        assert_eq!(URL_SAFE_NO_PAD.decode(new_challenge()).unwrap().len(), 32);
    }
}
