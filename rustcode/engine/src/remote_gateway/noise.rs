//! `Noise_NK_25519_ChaChaPoly_SHA256`, for sealed gateway connections.
//!
//! A device that knows the gateway's static X25519 public key (from the
//! pairing QR code) opens a sealed connection with one handshake message and
//! gets one back. Only the gateway can complete it, so a sealed connection
//! is encrypted and authenticates the gateway on any network, including a
//! plain LAN and a relay that copies the frames. The device still proves
//! itself inside, with its token or device key, exactly as on a plain socket.
//!
//! This follows revision 34 of the Noise specification with one difference:
//! a transport message is not capped at 65535 bytes, because the WebSocket
//! frame around it carries its length. Checked against the cacophony test
//! vector for this protocol name.

use anyhow::{Result, anyhow, bail, ensure};
use aws_lc_rs::aead::{Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::agreement::{self, PrivateKey, UnparsedPublicKey, X25519};
use aws_lc_rs::{digest, hmac};
use rand::RngExt as _;

pub const PROTOCOL_NAME: &str = "Noise_NK_25519_ChaChaPoly_SHA256";

/// Bound into every sealed handshake, so a transcript for anything else
/// using the same keys never completes as one.
pub const PROLOGUE: &[u8] = b"rustcode-remote-sealed-v1";

const KEY_BYTES: usize = 32;
const TAG_BYTES: usize = 16;

/// An X25519 key pair.
pub struct KeyPair {
    private: [u8; KEY_BYTES],
    public: [u8; KEY_BYTES],
}

impl KeyPair {
    pub fn generate() -> Self {
        let mut private = [0u8; KEY_BYTES];
        rand::rng().fill(&mut private);
        Self::from_private(private).expect("any 32 bytes are an X25519 private key")
    }

    pub fn from_private(private: [u8; KEY_BYTES]) -> Result<Self> {
        let key = PrivateKey::from_private_key(&X25519, &private)
            .map_err(|_| anyhow!("not an X25519 private key"))?;
        let public = key
            .compute_public_key()
            .map_err(|_| anyhow!("could not derive the X25519 public key"))?;
        let public: [u8; KEY_BYTES] = public
            .as_ref()
            .try_into()
            .map_err(|_| anyhow!("an X25519 public key is 32 bytes"))?;
        Ok(Self { private, public })
    }

    pub fn private(&self) -> &[u8; KEY_BYTES] {
        &self.private
    }

    pub fn public(&self) -> &[u8; KEY_BYTES] {
        &self.public
    }

    fn agree(&self, peer: &[u8]) -> Result<[u8; KEY_BYTES]> {
        ensure!(peer.len() == KEY_BYTES, "an X25519 public key is 32 bytes");
        let key = PrivateKey::from_private_key(&X25519, &self.private)
            .map_err(|_| anyhow!("not an X25519 private key"))?;
        agreement::agree(
            &key,
            UnparsedPublicKey::new(&X25519, peer),
            anyhow!("the peer's X25519 key is unusable"),
            |shared| {
                shared
                    .try_into()
                    .map_err(|_| anyhow!("an X25519 shared secret is 32 bytes"))
            },
        )
    }
}

fn hash(parts: &[&[u8]]) -> [u8; 32] {
    let mut context = digest::Context::new(&digest::SHA256);
    for part in parts {
        context.update(part);
    }
    context
        .finish()
        .as_ref()
        .try_into()
        .expect("SHA-256 is 32 bytes")
}

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let mut context = hmac::Context::with_key(&key);
    for part in parts {
        context.update(part);
    }
    context
        .sign()
        .as_ref()
        .try_into()
        .expect("HMAC-SHA-256 is 32 bytes")
}

/// Noise's two-output HKDF.
fn hkdf2(chaining_key: &[u8; 32], input: &[u8]) -> ([u8; 32], [u8; 32]) {
    let temp = hmac_sha256(chaining_key, &[input]);
    let first = hmac_sha256(&temp, &[&[1]]);
    let second = hmac_sha256(&temp, &[&first, &[2]]);
    (first, second)
}

fn nonce(counter: u64) -> Nonce {
    let mut bytes = [0u8; 12];
    bytes[4..].copy_from_slice(&counter.to_le_bytes());
    Nonce::assume_unique_for_key(bytes)
}

/// One direction's key and message counter.
pub struct CipherState {
    key: LessSafeKey,
    counter: u64,
}

impl CipherState {
    fn new(key: &[u8; 32]) -> Self {
        let key = UnboundKey::new(&CHACHA20_POLY1305, key).expect("a 32-byte ChaCha20 key");
        Self {
            key: LessSafeKey::new(key),
            counter: 0,
        }
    }

    fn next_nonce(&mut self) -> Result<Nonce> {
        ensure!(
            self.counter < u64::MAX,
            "the sealed connection used up its nonces"
        );
        let nonce = nonce(self.counter);
        self.counter += 1;
        Ok(nonce)
    }

    fn seal(&mut self, associated: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = self.next_nonce()?;
        let mut buffer = Vec::with_capacity(plaintext.len() + TAG_BYTES);
        buffer.extend_from_slice(plaintext);
        self.key
            .seal_in_place_append_tag(nonce, Aad::from(associated), &mut buffer)
            .map_err(|_| anyhow!("could not seal a frame"))?;
        Ok(buffer)
    }

    fn open(&mut self, associated: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        ensure!(ciphertext.len() >= TAG_BYTES, "a sealed frame is too short");
        let nonce = self.next_nonce()?;
        let mut buffer = ciphertext.to_vec();
        let plaintext = self
            .key
            .open_in_place(nonce, Aad::from(associated), &mut buffer)
            .map_err(|_| anyhow!("a sealed frame did not authenticate"))?;
        let length = plaintext.len();
        buffer.truncate(length);
        Ok(buffer)
    }

    /// Seal a transport message.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        self.seal(&[], plaintext)
    }

    /// Open a transport message. A failure leaves the connection unusable:
    /// the counters no longer agree.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        self.open(&[], ciphertext)
    }
}

/// Noise's symmetric state during the handshake.
struct Symmetric {
    chaining_key: [u8; 32],
    hash: [u8; 32],
    cipher: Option<CipherState>,
}

impl Symmetric {
    fn new(prologue: &[u8]) -> Self {
        let name: [u8; 32] = PROTOCOL_NAME
            .as_bytes()
            .try_into()
            .expect("the protocol name is exactly 32 bytes");
        let mut state = Self {
            chaining_key: name,
            hash: name,
            cipher: None,
        };
        state.mix_hash(prologue);
        state
    }

    fn mix_hash(&mut self, data: &[u8]) {
        self.hash = hash(&[&self.hash, data]);
    }

    fn mix_key(&mut self, input: &[u8]) {
        let (chaining_key, key) = hkdf2(&self.chaining_key, input);
        self.chaining_key = chaining_key;
        self.cipher = Some(CipherState::new(&key));
    }

    fn encrypt_and_hash(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let ciphertext = match self.cipher.as_mut() {
            Some(cipher) => cipher.seal(&self.hash, plaintext)?,
            None => plaintext.to_vec(),
        };
        self.mix_hash(&ciphertext);
        Ok(ciphertext)
    }

    fn decrypt_and_hash(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let plaintext = match self.cipher.as_mut() {
            Some(cipher) => cipher.open(&self.hash, ciphertext)?,
            None => ciphertext.to_vec(),
        };
        self.mix_hash(ciphertext);
        Ok(plaintext)
    }

    fn split(&self) -> (CipherState, CipherState) {
        let (first, second) = hkdf2(&self.chaining_key, &[]);
        (CipherState::new(&first), CipherState::new(&second))
    }
}

/// Both directions of a finished handshake.
pub struct Transport {
    pub send: CipherState,
    pub receive: CipherState,
    /// The handshake hash, the same on both ends.
    pub handshake_hash: [u8; 32],
}

/// What a responder learns from the first message and answers with.
pub struct Responded {
    pub payload: Vec<u8>,
    pub message: Vec<u8>,
    pub transport: Transport,
}

/// Answer an initiator's first message with a fresh ephemeral key.
pub fn respond(
    static_key: &KeyPair,
    prologue: &[u8],
    message: &[u8],
    payload: &[u8],
) -> Result<Responded> {
    respond_with(static_key, &KeyPair::generate(), prologue, message, payload)
}

fn respond_with(
    static_key: &KeyPair,
    ephemeral: &KeyPair,
    prologue: &[u8],
    message: &[u8],
    payload: &[u8],
) -> Result<Responded> {
    if message.len() < KEY_BYTES + TAG_BYTES {
        bail!("the sealed handshake message is too short");
    }
    let mut state = Symmetric::new(prologue);
    state.mix_hash(static_key.public());
    let (initiator_ephemeral, rest) = message.split_at(KEY_BYTES);
    state.mix_hash(initiator_ephemeral);
    state.mix_key(&static_key.agree(initiator_ephemeral)?);
    let received = state.decrypt_and_hash(rest)?;
    let mut answer = ephemeral.public().to_vec();
    state.mix_hash(ephemeral.public());
    state.mix_key(&ephemeral.agree(initiator_ephemeral)?);
    answer.extend(state.encrypt_and_hash(payload)?);
    let (to_responder, to_initiator) = state.split();
    Ok(Responded {
        payload: received,
        message: answer,
        transport: Transport {
            send: to_initiator,
            receive: to_responder,
            handshake_hash: state.hash,
        },
    })
}

/// An initiator waiting for the responder's answer.
pub struct Initiator {
    state: Symmetric,
    ephemeral: KeyPair,
}

impl Initiator {
    /// Start a handshake with the responder whose static key is
    /// `remote_static`; returns the first message.
    pub fn start(remote_static: &[u8], prologue: &[u8], payload: &[u8]) -> Result<(Self, Vec<u8>)> {
        Self::start_with(KeyPair::generate(), remote_static, prologue, payload)
    }

    fn start_with(
        ephemeral: KeyPair,
        remote_static: &[u8],
        prologue: &[u8],
        payload: &[u8],
    ) -> Result<(Self, Vec<u8>)> {
        ensure!(
            remote_static.len() == KEY_BYTES,
            "an X25519 public key is 32 bytes"
        );
        let mut state = Symmetric::new(prologue);
        state.mix_hash(remote_static);
        let mut message = ephemeral.public().to_vec();
        state.mix_hash(ephemeral.public());
        state.mix_key(&ephemeral.agree(remote_static)?);
        message.extend(state.encrypt_and_hash(payload)?);
        Ok((Self { state, ephemeral }, message))
    }

    /// Read the responder's answer; returns its payload and the transport.
    pub fn finish(mut self, message: &[u8]) -> Result<(Vec<u8>, Transport)> {
        if message.len() < KEY_BYTES + TAG_BYTES {
            bail!("the sealed handshake answer is too short");
        }
        let (responder_ephemeral, rest) = message.split_at(KEY_BYTES);
        self.state.mix_hash(responder_ephemeral);
        self.state
            .mix_key(&self.ephemeral.agree(responder_ephemeral)?);
        let payload = self.state.decrypt_and_hash(rest)?;
        let (to_responder, to_initiator) = self.state.split();
        Ok((
            payload,
            Transport {
                send: to_responder,
                receive: to_initiator,
                handshake_hash: self.state.hash,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(text: &str) -> Vec<u8> {
        hex::decode(text).unwrap()
    }

    fn key(text: &str) -> KeyPair {
        KeyPair::from_private(bytes(text).try_into().unwrap()).unwrap()
    }

    /// cacophony's vector for `Noise_NK_25519_ChaChaPoly_SHA256`.
    #[test]
    fn the_handshake_matches_the_published_test_vector() {
        let prologue = bytes("4a6f686e2047616c74");
        let responder_static =
            key("4a3acbfdb163dec651dfa3194dece676d437029c62a408b4c5ea9114246e4893");
        let initiator_ephemeral =
            key("893e28b9dc6ca8d611ab664754b8ceb7bac5117349a4439a6b0569da977c464a");
        let responder_ephemeral =
            key("bbdb4cdbd309f1a1f2e1456967fe288cadd6f712d65dc7b7793d5e63da6b375b");
        assert_eq!(
            responder_static.public().to_vec(),
            bytes("31e0303fd6418d2f8c0e78b91f22e8caed0fbe48656dcf4767e4834f701b8f62")
        );

        let (initiator, first) = Initiator::start_with(
            initiator_ephemeral,
            responder_static.public(),
            &prologue,
            &bytes("4c756477696720766f6e204d69736573"),
        )
        .unwrap();
        assert_eq!(
            first,
            bytes(
                "ca35def5ae56cec33dc2036731ab14896bc4c75dbb07a61f879f8e3afa4c79448134d00711fdb390a0d178fa008f6d47d2891e5ea18ae136c3b4c23ac384efb0"
            )
        );

        let responded = respond_with(
            &responder_static,
            &responder_ephemeral,
            &prologue,
            &first,
            &bytes("4d757272617920526f746862617264"),
        )
        .unwrap();
        assert_eq!(responded.payload, bytes("4c756477696720766f6e204d69736573"));
        assert_eq!(
            responded.message,
            bytes(
                "95ebc60d2b1fa672c1f46a8aa265ef51bfe38e7ccb39ec5be34069f1448088438ea16e3701bc0d77744f117bee22451c9afa7f4cdbbcff00c04a8ee0913c88"
            )
        );

        let (payload, mut initiator) = initiator.finish(&responded.message).unwrap();
        assert_eq!(payload, bytes("4d757272617920526f746862617264"));
        let mut responder = responded.transport;
        let handshake_hash =
            bytes("2efa38a9c7c93ac98f3a097af25c2f58b9e7673787717bc27e98827118c2c1a5");
        assert_eq!(initiator.handshake_hash.to_vec(), handshake_hash);
        assert_eq!(responder.handshake_hash.to_vec(), handshake_hash);

        let third = initiator
            .send
            .encrypt(&bytes("462e20412e20486179656b"))
            .unwrap();
        assert_eq!(
            third,
            bytes("a62de29ce27cb80245d440d986ed816c156e9d757d7008df2198b0")
        );
        assert_eq!(
            responder.receive.decrypt(&third).unwrap(),
            bytes("462e20412e20486179656b")
        );
        let fourth = responder
            .send
            .encrypt(&bytes("4361726c204d656e676572"))
            .unwrap();
        assert_eq!(
            fourth,
            bytes("174a35f11c689f4530d7208618e0564ae12f2f50ba8eb4df5382ff")
        );
        assert_eq!(
            initiator.receive.decrypt(&fourth).unwrap(),
            bytes("4361726c204d656e676572")
        );
    }

    #[test]
    fn only_the_holder_of_the_static_key_can_answer() {
        let gateway = KeyPair::generate();
        let impostor = KeyPair::generate();
        let (_, first) = Initiator::start(gateway.public(), PROLOGUE, b"").unwrap();
        assert!(respond(&impostor, PROLOGUE, &first, b"").is_err());
        assert!(respond(&gateway, PROLOGUE, &first, b"").is_ok());
    }

    #[test]
    fn a_tampered_or_replayed_frame_is_refused() {
        let gateway = KeyPair::generate();
        let (initiator, first) = Initiator::start(gateway.public(), PROLOGUE, b"").unwrap();
        let responded = respond(&gateway, PROLOGUE, &first, b"").unwrap();
        let (_, mut device) = initiator.finish(&responded.message).unwrap();
        let mut host = responded.transport;
        let mut sealed = device.send.encrypt(b"{\"type\":\"pair\"}").unwrap();
        let replay = sealed.clone();
        sealed[0] ^= 1;
        assert!(host.receive.decrypt(&sealed).is_err());

        let (initiator, first) = Initiator::start(gateway.public(), PROLOGUE, b"").unwrap();
        let responded = respond(&gateway, PROLOGUE, &first, b"").unwrap();
        let (_, mut device) = initiator.finish(&responded.message).unwrap();
        let mut host = responded.transport;
        let frame = device.send.encrypt(b"one").unwrap();
        assert_eq!(host.receive.decrypt(&frame).unwrap(), b"one");
        assert!(host.receive.decrypt(&frame).is_err());
        assert!(host.receive.decrypt(&replay).is_err());
    }

    #[test]
    fn another_prologue_does_not_complete() {
        let gateway = KeyPair::generate();
        let (_, first) = Initiator::start(gateway.public(), b"something else", b"").unwrap();
        assert!(respond(&gateway, PROLOGUE, &first, b"").is_err());
    }
}
