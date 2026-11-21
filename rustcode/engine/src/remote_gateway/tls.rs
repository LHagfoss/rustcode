//! The gateway's own TLS: a self-signed certificate, pinned by fingerprint
//! (#2651, `docs/remote-security.md` › Direct TLS).
//!
//! The gateway port answers both plain `ws://` and TLS: the first byte of a
//! connection says which (a TLS record starts with `0x16`, which no HTTP
//! request line does). The certificate is made once per install and kept in
//! `remote/tls.json` (owner-only, like the gateway identity). Nothing vouches
//! for it but its fingerprint, which a client learns from a connect code, a
//! QR code or a `described` frame and then insists on: a pinned client never
//! dials that machine over plain `ws://` again, and refuses any other
//! certificate.

use crate::daemon::private_files::{publish_private_json, read_private_json};
use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::{
    self,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};

/// The certificate's subject name. Clients pin the fingerprint and ignore the
/// name, so it only has to be a valid DNS name for the TLS handshake.
pub const CERTIFICATE_NAME: &str = "rustcode-gateway";

/// What `remote/tls.json` holds: DER, base64.
#[derive(Serialize, Deserialize)]
struct Stored {
    certificate: String,
    private_key: String,
}

/// The gateway's certificate and the acceptor built from it.
pub struct TlsIdentity {
    acceptor: TlsAcceptor,
    fingerprint: String,
}

impl TlsIdentity {
    /// Read the certificate at `path`, creating it when there is none. A file
    /// that exists but cannot be read stops the gateway rather than replacing
    /// the certificate every pinned device knows.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        let stored = match read_private_json::<Stored>(path, 16 * 1024) {
            Ok(stored) => stored,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                let stored = generate()?;
                publish_private_json(path, &stored)?;
                stored
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read the gateway certificate {}; remove it to make a new one \
                         (devices that pinned the old one must pair again)",
                        path.display()
                    )
                });
            }
        };
        Self::from_stored(&stored)
            .with_context(|| format!("the gateway certificate {} is unusable", path.display()))
    }

    /// A fresh certificate that is never written anywhere, for tests.
    #[cfg(test)]
    pub fn ephemeral() -> Result<Self> {
        Self::from_stored(&generate()?)
    }

    fn from_stored(stored: &Stored) -> Result<Self> {
        let certificate = STANDARD.decode(&stored.certificate)?;
        let private_key = STANDARD.decode(&stored.private_key)?;
        if certificate.is_empty() || private_key.is_empty() {
            bail!("the certificate or its key is empty");
        }
        let fingerprint = fingerprint(&certificate);
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(certificate)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key)),
            )?;
        Ok(Self {
            acceptor: TlsAcceptor::from(Arc::new(config)),
            fingerprint,
        })
    }

    pub fn acceptor(&self) -> &TlsAcceptor {
        &self.acceptor
    }

    /// The SHA-256 of the certificate's DER, lowercase hex.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }
}

/// The SHA-256 of a certificate's DER, lowercase hex: what clients pin.
pub fn fingerprint(certificate_der: &[u8]) -> String {
    hex::encode(Sha256::digest(certificate_der))
}

/// A new self-signed P-256 certificate. rcgen's default validity runs from
/// 1975 to 4096: nothing renews it, and a device's wrong clock cannot fail it,
/// because clients check the fingerprint, not the dates.
fn generate() -> Result<Stored> {
    let key = rcgen::KeyPair::generate().context("could not make a key for the certificate")?;
    let mut params = rcgen::CertificateParams::new(vec![CERTIFICATE_NAME.to_owned()])?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "RustCode gateway");
    let certificate = params.self_signed(&key)?;
    Ok(Stored {
        certificate: STANDARD.encode(certificate.der()),
        private_key: STANDARD.encode(key.serialize_der()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_certificate_is_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote").join("tls.json");
        let first = TlsIdentity::load_or_create(&path).unwrap();
        let again = TlsIdentity::load_or_create(&path).unwrap();
        assert_eq!(first.fingerprint(), again.fingerprint());
        assert_eq!(first.fingerprint().len(), 64);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn an_unreadable_certificate_is_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tls.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert!(TlsIdentity::load_or_create(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not json");
    }

    #[test]
    fn two_installs_get_different_certificates() {
        let one = TlsIdentity::ephemeral().unwrap();
        let other = TlsIdentity::ephemeral().unwrap();
        assert_ne!(one.fingerprint(), other.fingerprint());
    }
}
