//! This install's Ed25519 key, for signing in to machines (issue #3956).
//!
//! A machine that lists `key_auth` gives every socket a challenge in its
//! `described` frame. This client sends its public key once, next to the token
//! it already holds; the machine answers `key_registered`, drops the token, and
//! from then on takes only this key's signature of the socket's challenge. A
//! signature is good for one socket, so one seen at a tunnel's edge cannot be
//! replayed.
//!
//! One key per install (configuration directory), kept in the same credential
//! store as the machine tokens ([`crate::tokens`]) under the slot
//! [`KEY_SLOT`]: unpadded base64url of its PKCS#8 document. The private key
//! never leaves this computer and is never logged.
//!
//! What is signed must match the gateway byte for byte
//! (`rustcode/engine/src/remote_gateway/device_key.rs`):
//! `rustcode-remote-auth-v1`, the gateway id, the challenge and the device id,
//! each followed by a zero byte.

use crate::protocol::Secret;
use crate::tokens::TokenStore;
use anyhow::{Context, Result};
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use std::fmt;
use std::time::Duration;

/// Separates these signatures from anything else the key could sign.
pub const SIGNATURE_DOMAIN: &str = "rustcode-remote-auth-v1";

/// The token store slot that holds the key. Not a machine's host id: those are
/// hex, so the two never meet.
pub const KEY_SLOT: &str = "device-key";

/// The bytes signed for `challenge` from the gateway `gateway_id`, as
/// `device_id`.
pub fn signed_message(gateway_id: &str, challenge: &str, device_id: &str) -> Vec<u8> {
    let mut message = Vec::new();
    for part in [SIGNATURE_DOMAIN, gateway_id, challenge, device_id] {
        message.extend_from_slice(part.as_bytes());
        message.push(0);
    }
    message
}

/// This install's key pair.
pub struct DeviceKey(Ed25519KeyPair);

impl DeviceKey {
    /// A new key and the PKCS#8 document to store it as.
    fn generate() -> Result<(Self, Secret)> {
        let document = Ed25519KeyPair::generate_pkcs8(&aws_lc_rs::rand::SystemRandom::new())
            .map_err(|_| anyhow::anyhow!("could not generate this computer's key"))?;
        let key = Self::from_document(document.as_ref())?;
        Ok((key, Secret::new(URL_SAFE_NO_PAD.encode(document.as_ref()))))
    }

    fn from_document(document: &[u8]) -> Result<Self> {
        Ed25519KeyPair::from_pkcs8(document)
            .map(Self)
            .map_err(|_| anyhow::anyhow!("this computer's stored key is not a valid key"))
    }

    fn from_stored(stored: &Secret) -> Result<Self> {
        let document = URL_SAFE_NO_PAD
            .decode(stored.expose())
            .context("this computer's stored key is not readable")?;
        Self::from_document(&document)
    }

    /// The key in `tokens`, if this install has one.
    pub fn load(tokens: &TokenStore) -> Result<Option<Self>> {
        tokens
            .get(KEY_SLOT)?
            .map(|stored| Self::from_stored(&stored))
            .transpose()
    }

    /// The key in `tokens`, made and stored first when this install has none.
    /// Two processes making one at once take turns under the store's lock, so
    /// both end up with the one that was stored.
    pub async fn load_or_create(tokens: &TokenStore, timeout: Duration) -> Result<Self> {
        if let Some(key) = Self::load(tokens)? {
            return Ok(key);
        }
        let _creating = tokens.presentation_lock(KEY_SLOT, timeout).await?;
        if let Some(key) = Self::load(tokens)? {
            return Ok(key);
        }
        let (key, stored) = Self::generate()?;
        tokens
            .set(KEY_SLOT, &stored)
            .context("could not store this computer's key")?;
        Ok(key)
    }

    /// The public key as a machine stores it: unpadded base64url of 32 bytes.
    pub fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0.public_key().as_ref())
    }

    /// The signature of `challenge` from `gateway_id`, as `device_id`.
    pub fn sign(&self, gateway_id: &str, challenge: &str, device_id: &str) -> String {
        let message = signed_message(gateway_id, challenge, device_id);
        URL_SAFE_NO_PAD.encode(self.0.sign(&message).as_ref())
    }
}

impl fmt::Debug for DeviceKey {
    /// The public half only.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeviceKey")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::{ED25519, UnparsedPublicKey};

    #[tokio::test]
    async fn the_key_is_made_once_and_signs_what_the_gateway_checks() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = TokenStore::in_file(dir.path()).unwrap();
        assert!(DeviceKey::load(&tokens).unwrap().is_none());
        let key = DeviceKey::load_or_create(&tokens, Duration::from_secs(5))
            .await
            .unwrap();
        let again = DeviceKey::load_or_create(&tokens, Duration::from_secs(5))
            .await
            .unwrap();
        assert_eq!(key.public_key(), again.public_key());

        let signature = URL_SAFE_NO_PAD
            .decode(key.sign("gw", "challenge", "dev"))
            .unwrap();
        let public = URL_SAFE_NO_PAD.decode(key.public_key()).unwrap();
        assert_eq!(public.len(), 32);
        let verifier = UnparsedPublicKey::new(&ED25519, &public);
        assert!(
            verifier
                .verify(&signed_message("gw", "challenge", "dev"), &signature)
                .is_ok()
        );
        assert!(
            verifier
                .verify(&signed_message("gw", "other", "dev"), &signature)
                .is_err()
        );
        assert!(!format!("{key:?}").contains(tokens.get(KEY_SLOT).unwrap().unwrap().expose()));
    }
}
