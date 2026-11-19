//! The machine owner's passkeys, which approve a hosted agent's sign-in on
//! the public sign-in page (#3954, `docs/architecture/hub-plan.md` phase 2).
//!
//! Only the local user adds one: `rustcode outside owner add-passkey` (or
//! Settings › Outside agents) writes a one-time enrollment ticket next to the
//! grants and prints a link to `<public_url>/owner/passkey`, where the browser
//! creates the credential. A passkey is bound to the public address: its
//! relying party is that host, and every check below is made here, in the
//! engine, with no service in between.
//!
//! - Registration: the client data says `webauthn.create`, the ticket's
//!   challenge and the public origin; the authenticator data's RP ID hash is
//!   the host's, the user was present and verified, and it carries the new
//!   credential's public key (ES256, EdDSA or RS256). Attestation is not
//!   checked (`none`): the ticket, which only the local user can make, is
//!   what says the key is the owner's.
//! - Approval: a single-use challenge from the consent page, `webauthn.get`,
//!   the same origin and RP ID, presence and verification, the signature over
//!   the authenticator data and the client data's hash, and a signature
//!   counter that only goes up when the authenticator keeps one.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const PASSKEY_FILE: &str = "owner-passkeys.json";
const ENROLLMENT_FILE: &str = "owner-passkey-enrollments.json";
/// How long an enrollment link can be used.
pub const ENROLLMENT_SECS: i64 = 600;
const MAX_ENROLLMENTS: usize = 5;
pub const MAX_PASSKEYS: usize = 10;
const MAX_NAME_CHARS: usize = 60;
/// How long the consent page's challenge can be answered.
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(300);
const MAX_CHALLENGES: usize = 64;
/// The largest client data or attestation the page may post.
pub const MAX_FIELD_BYTES: usize = 16 * 1024;

const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_ATTESTED: u8 = 0x40;

const ALG_ES256: i64 = -7;
const ALG_EDDSA: i64 = -8;
const ALG_RS256: i64 = -257;

/// The algorithms the page offers, best first.
pub const ALGORITHMS: [i64; 3] = [ALG_ES256, ALG_EDDSA, ALG_RS256];

// ---------------------------------------------------------------- storage

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kty", rename_all = "snake_case")]
enum PublicKey {
    /// P-256, as the uncompressed point.
    Ec2 {
        point: String,
    },
    /// Ed25519.
    Okp {
        x: String,
    },
    Rsa {
        n: String,
        e: String,
    },
}

/// One of the owner's passkeys. The credential's public key only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passkey {
    /// The credential id, base64url.
    pub id: String,
    pub name: String,
    /// The host the passkey was made for.
    pub rp_id: String,
    key: PublicKey,
    sign_count: u32,
    pub created_at: String,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

impl Passkey {
    /// What Settings and the CLI show: never the key.
    pub fn view(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "host": self.rp_id,
            "createdAt": self.created_at,
            "lastUsedAt": self.last_used_at,
        })
    }
}

#[derive(Default, Serialize, Deserialize)]
struct PasskeyStore {
    #[serde(default)]
    passkeys: Vec<Passkey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Enrollment {
    hash: String,
    challenge: String,
    user: String,
    name: String,
    expires_at: i64,
}

#[derive(Default, Serialize, Deserialize)]
struct EnrollmentStore {
    #[serde(default)]
    enrollments: Vec<Enrollment>,
}

static STORE_LOCK: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    STORE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn passkey_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(PASSKEY_FILE))
}

pub fn enrollment_path() -> Option<PathBuf> {
    crate::config::get_config_dir().map(|dir| dir.join(ENROLLMENT_FILE))
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    crate::service::write_private(path, &bytes).map_err(|error| error.to_string())
}

pub fn list_at(path: &Path) -> Vec<Passkey> {
    read_json::<PasskeyStore>(path).passkeys
}

/// Every passkey of this machine's owner.
pub fn list() -> Vec<Passkey> {
    passkey_path()
        .map(|path| list_at(&path))
        .unwrap_or_default()
}

/// Remove the passkey `selector` names: its id, a prefix of it, or its name.
pub fn remove_at(path: &Path, selector: &str) -> Result<Passkey, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("name the passkey to remove".into());
    }
    let _guard = lock();
    let mut store = read_json::<PasskeyStore>(path);
    let matches: Vec<usize> = store
        .passkeys
        .iter()
        .enumerate()
        .filter(|(_, passkey)| {
            passkey.id == selector
                || passkey.id.starts_with(selector)
                || passkey.name.eq_ignore_ascii_case(selector)
        })
        .map(|(index, _)| index)
        .collect();
    let index = match matches.as_slice() {
        [one] => *one,
        [] => return Err(format!("no passkey matches {selector}")),
        _ => {
            return Err(format!(
                "more than one passkey matches {selector}; use its id"
            ));
        }
    };
    let removed = store.passkeys.remove(index);
    write_json(path, &store)?;
    Ok(removed)
}

pub fn remove(selector: &str) -> Result<Passkey, String> {
    remove_at(
        &passkey_path().ok_or("configuration unavailable")?,
        selector,
    )
}

/// The link that adds a passkey named `name` on this machine's public
/// sign-in page, for [`ENROLLMENT_SECS`]. Needs `[outside] public_url`: a
/// passkey belongs to that address.
pub fn add_link(name: &str) -> Result<String, String> {
    let base = match super::outside_http::configured_public_base() {
        Some(Ok(base)) => base,
        Some(Err(error)) => return Err(error),
        None => {
            return Err(
                "no [outside] public_url is set in config.toml; a passkey belongs to the public sign-in page (docs/mcp-outside-agents.md › Hosted agents)".into(),
            );
        }
    };
    let ticket = create_enrollment_at(
        &enrollment_path().ok_or("configuration unavailable")?,
        &passkey_path().ok_or("configuration unavailable")?,
        name,
    )?;
    Ok(format!("{base}/owner/passkey?ticket={ticket}"))
}

fn random_b64(bytes: usize) -> String {
    let mut out = Vec::with_capacity(bytes);
    while out.len() < bytes {
        out.extend_from_slice(&rand::random::<u128>().to_le_bytes());
    }
    out.truncate(bytes);
    URL_SAFE_NO_PAD.encode(out)
}

fn hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// A one-time ticket that lets one browser add a passkey named `name`, for
/// [`ENROLLMENT_SECS`]. Only its hash is kept.
pub fn create_enrollment_at(path: &Path, passkeys: &Path, name: &str) -> Result<String, String> {
    let name: String = name.trim().chars().take(MAX_NAME_CHARS).collect();
    let name = if name.is_empty() {
        "Passkey".to_owned()
    } else {
        name
    };
    if list_at(passkeys).len() >= MAX_PASSKEYS {
        return Err(format!(
            "this machine already has {MAX_PASSKEYS} passkeys; remove one first"
        ));
    }
    let ticket = random_b64(32);
    let _guard = lock();
    let mut store = read_json::<EnrollmentStore>(path);
    let now = now();
    store.enrollments.retain(|item| item.expires_at > now);
    if store.enrollments.len() >= MAX_ENROLLMENTS {
        store.enrollments.sort_by_key(|item| item.expires_at);
        store.enrollments.remove(0);
    }
    store.enrollments.push(Enrollment {
        hash: hash(&ticket),
        challenge: random_b64(32),
        user: random_b64(16),
        name,
        expires_at: now + ENROLLMENT_SECS,
    });
    write_json(path, &store)?;
    Ok(ticket)
}

/// What the enrollment page needs: the challenge, the user handle and the name.
pub(super) struct Offer {
    pub challenge: String,
    pub user: String,
    pub name: String,
}

/// The open enrollment `ticket` belongs to, without spending it.
pub(super) fn enrollment_at(path: &Path, ticket: &str) -> Option<Offer> {
    let wanted = hash(ticket.trim());
    let now = now();
    read_json::<EnrollmentStore>(path)
        .enrollments
        .into_iter()
        .find(|item| item.expires_at > now && super::outside::same(&item.hash, &wanted))
        .map(|item| Offer {
            challenge: item.challenge,
            user: item.user,
            name: item.name,
        })
}

fn take_enrollment(path: &Path, ticket: &str) -> Option<Enrollment> {
    let wanted = hash(ticket.trim());
    let now = now();
    let _guard = lock();
    let mut store = read_json::<EnrollmentStore>(path);
    let index = store
        .enrollments
        .iter()
        .position(|item| item.expires_at > now && super::outside::same(&item.hash, &wanted))?;
    let item = store.enrollments.remove(index);
    write_json(path, &store).ok()?;
    Some(item)
}

// ------------------------------------------------------------ challenges

static CHALLENGES: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

/// A fresh challenge for one consent page; it answers one assertion.
pub(super) fn new_challenge() -> String {
    let challenge = random_b64(32);
    let mut open = CHALLENGES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    open.retain(|(_, expires)| *expires > now);
    if open.len() >= MAX_CHALLENGES {
        open.remove(0);
    }
    open.push((challenge.clone(), now + CHALLENGE_LIFETIME));
    challenge
}

fn spend_challenge(challenge: &str) -> bool {
    let mut open = CHALLENGES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    open.retain(|(_, expires)| *expires > now);
    match open
        .iter()
        .position(|(open, _)| super::outside::same(open, challenge))
    {
        Some(index) => {
            open.remove(index);
            true
        }
        None => false,
    }
}

// -------------------------------------------------------------- the checks

/// The relying party of a public base: its host.
pub fn rp_id(origin: &str) -> String {
    url::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn decode_field(text: &str, what: &str) -> Result<Vec<u8>, String> {
    if text.len() > MAX_FIELD_BYTES * 2 {
        return Err(format!("the {what} is too large"));
    }
    URL_SAFE_NO_PAD
        .decode(text.trim().trim_end_matches('='))
        .map_err(|_| format!("the {what} is not base64url"))
}

/// The client data, checked against the ceremony, the challenge and the
/// origin. Returns the challenge it carried.
fn client_data(json: &[u8], ceremony: &str, origin: &str) -> Result<String, String> {
    let data: Value =
        serde_json::from_slice(json).map_err(|_| "the client data is not JSON".to_owned())?;
    if data.get("type").and_then(Value::as_str) != Some(ceremony) {
        return Err("the browser answered a different request".into());
    }
    if data.get("origin").and_then(Value::as_str) != Some(origin) {
        return Err("the passkey was used on a different site".into());
    }
    if data.get("crossOrigin").and_then(Value::as_bool) == Some(true) {
        return Err("the passkey was used from inside another site".into());
    }
    data.get("challenge")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "the client data has no challenge".into())
}

struct AuthData<'a> {
    flags: u8,
    sign_count: u32,
    /// The attested credential data and what follows it.
    rest: &'a [u8],
}

fn auth_data<'a>(data: &'a [u8], rp_id: &str) -> Result<AuthData<'a>, String> {
    if data.len() < 37 {
        return Err("the authenticator data is too short".into());
    }
    if data[..32] != Sha256::digest(rp_id.as_bytes())[..] {
        return Err("the passkey belongs to a different site".into());
    }
    let flags = data[32];
    if flags & FLAG_USER_PRESENT == 0 || flags & FLAG_USER_VERIFIED == 0 {
        return Err("the passkey did not verify you (no PIN, fingerprint or face)".into());
    }
    Ok(AuthData {
        flags,
        sign_count: u32::from_be_bytes([data[33], data[34], data[35], data[36]]),
        rest: &data[37..],
    })
}

fn bytes_of(map: &[(Cbor, Cbor)], key: i64) -> Option<Vec<u8>> {
    map.iter().find_map(|(k, v)| match (k.as_int(), v) {
        (Some(found), Cbor::Bytes(bytes)) if found == key => Some(bytes.clone()),
        _ => None,
    })
}

fn int_of(map: &[(Cbor, Cbor)], key: i64) -> Option<i64> {
    map.iter()
        .find_map(|(k, v)| (k.as_int() == Some(key)).then(|| v.as_int()).flatten())
}

fn cose_key(value: &Cbor) -> Result<PublicKey, String> {
    let Cbor::Map(map) = value else {
        return Err("the credential key is not a COSE key".into());
    };
    let unsupported = || "the passkey uses a key type RustCode does not support".to_owned();
    match (int_of(map, 1), int_of(map, 3)) {
        (Some(2), Some(ALG_ES256)) => {
            let (Some(1), Some(x), Some(y)) =
                (int_of(map, -1), bytes_of(map, -2), bytes_of(map, -3))
            else {
                return Err(unsupported());
            };
            if x.len() != 32 || y.len() != 32 {
                return Err(unsupported());
            }
            let mut point = vec![0x04];
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            Ok(PublicKey::Ec2 {
                point: URL_SAFE_NO_PAD.encode(point),
            })
        }
        (Some(1), Some(ALG_EDDSA)) => match (int_of(map, -1), bytes_of(map, -2)) {
            (Some(6), Some(x)) if x.len() == 32 => Ok(PublicKey::Okp {
                x: URL_SAFE_NO_PAD.encode(x),
            }),
            _ => Err(unsupported()),
        },
        (Some(3), Some(ALG_RS256)) => match (bytes_of(map, -1), bytes_of(map, -2)) {
            (Some(n), Some(e)) if n.len() >= 256 => Ok(PublicKey::Rsa {
                n: URL_SAFE_NO_PAD.encode(n),
                e: URL_SAFE_NO_PAD.encode(e),
            }),
            _ => Err(unsupported()),
        },
        _ => Err(unsupported()),
    }
}

fn verify(key: &PublicKey, message: &[u8], signature: &[u8]) -> bool {
    use aws_lc_rs::signature::{
        ECDSA_P256_SHA256_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents,
        UnparsedPublicKey,
    };
    let decode = |text: &str| URL_SAFE_NO_PAD.decode(text).unwrap_or_default();
    match key {
        PublicKey::Ec2 { point } => UnparsedPublicKey::new(&ECDSA_P256_SHA256_ASN1, decode(point))
            .verify(message, signature)
            .is_ok(),
        PublicKey::Okp { x } => UnparsedPublicKey::new(&ED25519, decode(x))
            .verify(message, signature)
            .is_ok(),
        PublicKey::Rsa { n, e } => {
            let (n, e) = (decode(n), decode(e));
            RsaPublicKeyComponents { n: &n, e: &e }
                .verify(&RSA_PKCS1_2048_8192_SHA256, message, signature)
                .is_ok()
        }
    }
}

/// What the enrollment page posts.
pub(super) struct Registration<'a> {
    pub ticket: &'a str,
    pub client_data: &'a str,
    pub attestation: &'a str,
}

/// Add the passkey the browser made, if every check holds. The ticket is
/// spent by this call either way.
pub(super) fn register_at(
    passkeys: &Path,
    enrollments: &Path,
    origin: &str,
    posted: &Registration<'_>,
) -> Result<Passkey, String> {
    let enrollment = take_enrollment(enrollments, posted.ticket).ok_or_else(|| {
        "this link was used already or has expired; make a new one with `rustcode outside owner add-passkey`".to_owned()
    })?;
    let client = decode_field(posted.client_data, "client data")?;
    let challenge = client_data(&client, "webauthn.create", origin)?;
    if !super::outside::same(&challenge, &enrollment.challenge) {
        return Err("the browser answered a different request".into());
    }
    let attestation = decode_field(posted.attestation, "attestation")?;
    let (object, _) = Cbor::decode(&attestation)?;
    let Cbor::Map(object) = object else {
        return Err("the attestation is not a map".into());
    };
    let data = object
        .iter()
        .find_map(|(k, v)| match (k, v) {
            (Cbor::Text(key), Cbor::Bytes(bytes)) if key == "authData" => Some(bytes.clone()),
            _ => None,
        })
        .ok_or("the attestation has no authenticator data")?;
    let rp = rp_id(origin);
    let parsed = auth_data(&data, &rp)?;
    if parsed.flags & FLAG_ATTESTED == 0 || parsed.rest.len() < 18 {
        return Err("the authenticator sent no credential".into());
    }
    let length = u16::from_be_bytes([parsed.rest[16], parsed.rest[17]]) as usize;
    let id = parsed
        .rest
        .get(18..18 + length)
        .ok_or("the credential id is cut short")?;
    let (key, _) = Cbor::decode(&parsed.rest[18 + length..])?;
    let key = cose_key(&key)?;
    let passkey = Passkey {
        id: URL_SAFE_NO_PAD.encode(id),
        name: enrollment.name,
        rp_id: rp,
        key,
        sign_count: parsed.sign_count,
        created_at: chrono::Utc::now().to_rfc3339(),
        last_used_at: None,
    };
    let _guard = lock();
    let mut store = read_json::<PasskeyStore>(passkeys);
    if store.passkeys.iter().any(|known| known.id == passkey.id) {
        return Err("this passkey is already added".into());
    }
    if store.passkeys.len() >= MAX_PASSKEYS {
        return Err(format!(
            "this machine already has {MAX_PASSKEYS} passkeys; remove one first"
        ));
    }
    store.passkeys.push(passkey.clone());
    write_json(passkeys, &store)?;
    Ok(passkey)
}

/// What the consent page posts after the browser signed.
pub(super) struct Assertion<'a> {
    pub id: &'a str,
    pub client_data: &'a str,
    pub authenticator_data: &'a str,
    pub signature: &'a str,
}

/// Check one approval. Returns the passkey's name.
pub(super) fn verify_assertion_at(
    passkeys: &Path,
    origin: &str,
    posted: &Assertion<'_>,
) -> Result<String, String> {
    let client = decode_field(posted.client_data, "client data")?;
    let challenge = client_data(&client, "webauthn.get", origin)?;
    if !spend_challenge(&challenge) {
        return Err("this approval was used already or has expired; reload the page".into());
    }
    let data = decode_field(posted.authenticator_data, "authenticator data")?;
    let rp = rp_id(origin);
    let parsed = auth_data(&data, &rp)?;
    let signature = decode_field(posted.signature, "signature")?;
    let _guard = lock();
    let mut store = read_json::<PasskeyStore>(passkeys);
    let passkey = store
        .passkeys
        .iter_mut()
        .find(|known| known.id == posted.id.trim() && known.rp_id == rp)
        .ok_or("that passkey is not one of this machine's owner's")?;
    let mut message = data.clone();
    message.extend_from_slice(&Sha256::digest(&client));
    if !verify(&passkey.key, &message, &signature) {
        return Err("the passkey's signature does not check out".into());
    }
    // An authenticator that keeps a counter moves it on every use; one that
    // stands still or goes back is a copy, or the same answer sent again.
    if (parsed.sign_count != 0 || passkey.sign_count != 0)
        && parsed.sign_count <= passkey.sign_count
    {
        return Err("the passkey's counter went backwards; it may have been copied".into());
    }
    passkey.sign_count = parsed.sign_count;
    passkey.last_used_at = Some(chrono::Utc::now().to_rfc3339());
    let name = passkey.name.clone();
    write_json(passkeys, &store)?;
    Ok(name)
}

// --------------------------------------------------------------- the pages

/// The script both pages share: base64url both ways.
const B64: &str = "const b=s=>Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')+'==='.slice((s.length+3)%4)),c=>c.charCodeAt(0));const e=a=>btoa(String.fromCharCode(...new Uint8Array(a))).replace(/\\+/g,'-').replace(/\\//g,'_').replace(/=+$/,'');";

/// `value` as a JavaScript literal that cannot end the script it is in.
fn js<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value)
        .unwrap_or_else(|_| "null".into())
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// The headers of a page with one inline script, allowed by its nonce.
pub(super) fn script_headers(nonce: &str) -> String {
    format!(
        "Content-Security-Policy: default-src 'none'; script-src 'nonce-{nonce}'; style-src 'unsafe-inline'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'\r\nX-Frame-Options: DENY\r\nReferrer-Policy: same-origin\r\n"
    )
}

pub(super) fn nonce() -> String {
    random_b64(18)
}

/// What the consent page needs to offer the owner's passkeys.
pub(super) struct PasskeyPrompt {
    pub challenge: String,
    pub rp_id: String,
    pub ids: Vec<String>,
    pub nonce: String,
}

impl PasskeyPrompt {
    /// A prompt for the passkeys made for `origin`'s host, if there are any.
    pub(super) fn for_origin(passkeys: &Path, origin: &str) -> Option<Self> {
        let rp = rp_id(origin);
        let ids: Vec<String> = list_at(passkeys)
            .into_iter()
            .filter(|passkey| passkey.rp_id == rp)
            .map(|passkey| passkey.id)
            .collect();
        (!ids.is_empty()).then(|| Self {
            challenge: new_challenge(),
            rp_id: rp,
            ids,
            nonce: nonce(),
        })
    }

    /// The button, its hidden fields and the script, inside the consent form.
    pub(super) fn controls(&self) -> String {
        let ids = js(&self.ids);
        let challenge = js(&self.challenge);
        let rp = js(&self.rp_id);
        format!(
            r#"<input type="hidden" name="pk_id"><input type="hidden" name="pk_client"><input type="hidden" name="pk_auth"><input type="hidden" name="pk_sig"><div class="actions"><button type="button" class="primary" id="passkey" style="flex:1">Approve with passkey</button></div><p class="hint" id="passkey-error" role="alert" hidden></p><script nonce="{nonce}">{B64}document.getElementById('passkey').addEventListener('click',async()=>{{const f=document.getElementById('passkey').form;const say=t=>{{const p=document.getElementById('passkey-error');p.textContent=t;p.hidden=false;}};try{{const c=await navigator.credentials.get({{publicKey:{{challenge:b({challenge}),rpId:{rp},userVerification:'required',allowCredentials:{ids}.map(i=>({{type:'public-key',id:b(i)}}))}}}});f.pk_id.value=e(c.rawId);f.pk_client.value=e(c.response.clientDataJSON);f.pk_auth.value=e(c.response.authenticatorData);f.pk_sig.value=e(c.response.signature);const d=document.createElement('input');d.type='hidden';d.name='decision';d.value='passkey';f.appendChild(d);f.submit();}}catch(x){{say('The passkey did not answer. Try again, or use a sign-in code.');}}}});</script>"#,
            nonce = self.nonce,
        )
    }
}

/// The page that adds a passkey for the owner, from an enrollment link.
pub(super) fn enrollment_page(
    machine: &str,
    ticket: &str,
    offer: &Offer,
    rp_id: &str,
    existing: &[String],
    nonce: &str,
) -> String {
    use super::outside_signin::escape;
    let challenge = js(&offer.challenge);
    let user = js(&offer.user);
    let name = js(&offer.name);
    let rp = js(rp_id);
    let exclude = js(existing);
    let algorithms = js(&ALGORITHMS);
    let body = format!(
        r#"<div class="pair"><div class="avatar"><img src="/logo.png" alt="RustCode"></div></div><p class="eyebrow">RustCode on {machine}</p><h1>Add a passkey</h1><p class="sub">“{label}” will approve hosted agents that sign in to RustCode on {machine} at <b>{host}</b>, with no sign-in code. Only add it on a device you keep.</p><form method="post" action="/owner/passkey" id="enroll"><input type="hidden" name="ticket" value="{ticket}"><input type="hidden" name="pk_client"><input type="hidden" name="pk_attestation"><div class="actions"><button type="button" class="primary" id="add" style="flex:1">Add passkey</button></div><p class="hint" id="add-error" role="alert" hidden></p></form><p class="foot">Remove it any time with <code>rustcode outside owner remove-passkey</code> or in RustCode › Settings › Outside agents.</p><script nonce="{nonce}">{B64}document.getElementById('add').addEventListener('click',async()=>{{const f=document.getElementById('enroll');const say=t=>{{const p=document.getElementById('add-error');p.textContent=t;p.hidden=false;}};try{{const c=await navigator.credentials.create({{publicKey:{{challenge:b({challenge}),rp:{{id:{rp},name:'RustCode'}},user:{{id:b({user}),name:'owner',displayName:{name}}},pubKeyCredParams:{algorithms}.map(a=>({{type:'public-key',alg:a}})),excludeCredentials:{exclude}.map(i=>({{type:'public-key',id:b(i)}})),authenticatorSelection:{{residentKey:'preferred',userVerification:'required'}},attestation:'none'}}}});f.pk_client.value=e(c.response.clientDataJSON);f.pk_attestation.value=e(c.response.attestationObject);f.submit();}}catch(x){{say('No passkey was made. Try again, or make a new link if this one expired.');}}}});</script>"#,
        machine = escape(machine),
        label = escape(&offer.name),
        host = escape(rp_id),
        ticket = escape(ticket),
    );
    super::outside_signin::page("Add a passkey to RustCode", "", &body)
}

// ------------------------------------------------------------------ CBOR

/// The part of CBOR (RFC 8949) WebAuthn uses: definite lengths, no floats
/// that matter, nesting bounded.
#[derive(Debug, Clone, PartialEq)]
enum Cbor {
    Unsigned(u64),
    Negative(i128),
    Bytes(Vec<u8>),
    Text(String),
    Array(Vec<Cbor>),
    Map(Vec<(Cbor, Cbor)>),
    Simple,
}

const MAX_DEPTH: usize = 16;
const MAX_ITEMS: u64 = 4096;

impl Cbor {
    fn as_int(&self) -> Option<i64> {
        match self {
            Self::Unsigned(value) => i64::try_from(*value).ok(),
            Self::Negative(value) => i64::try_from(*value).ok(),
            _ => None,
        }
    }

    /// One item from the start of `input`, and how many bytes it took.
    fn decode(input: &[u8]) -> Result<(Self, usize), String> {
        let mut at = 0;
        let value = Self::item(input, &mut at, 0)?;
        Ok((value, at))
    }

    fn item(input: &[u8], at: &mut usize, depth: usize) -> Result<Self, String> {
        let bad = || "the authenticator sent malformed CBOR".to_owned();
        if depth > MAX_DEPTH {
            return Err(bad());
        }
        let first = *input.get(*at).ok_or_else(bad)?;
        *at += 1;
        let major = first >> 5;
        let info = first & 0x1f;
        let mut take = |count: usize| -> Result<u64, String> {
            let bytes = input.get(*at..*at + count).ok_or_else(bad)?;
            *at += count;
            Ok(bytes
                .iter()
                .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
        };
        let argument = match info {
            0..=23 => u64::from(info),
            24 => take(1)?,
            25 => take(2)?,
            26 => take(4)?,
            27 => take(8)?,
            _ => return Err(bad()),
        };
        let mut slice = |length: u64| -> Result<Vec<u8>, String> {
            let length = usize::try_from(length).map_err(|_| bad())?;
            let bytes = input
                .get(*at..at.checked_add(length).ok_or_else(bad)?)
                .ok_or_else(bad)?;
            *at += length;
            Ok(bytes.to_vec())
        };
        Ok(match major {
            0 => Self::Unsigned(argument),
            1 => Self::Negative(-1 - i128::from(argument)),
            2 => Self::Bytes(slice(argument)?),
            3 => Self::Text(String::from_utf8(slice(argument)?).map_err(|_| bad())?),
            4 => {
                if argument > MAX_ITEMS {
                    return Err(bad());
                }
                let mut items = Vec::new();
                for _ in 0..argument {
                    items.push(Self::item(input, at, depth + 1)?);
                }
                Self::Array(items)
            }
            5 => {
                if argument > MAX_ITEMS {
                    return Err(bad());
                }
                let mut entries = Vec::new();
                for _ in 0..argument {
                    let key = Self::item(input, at, depth + 1)?;
                    let value = Self::item(input, at, depth + 1)?;
                    entries.push((key, value));
                }
                Self::Map(entries)
            }
            6 => Self::item(input, at, depth + 1)?,
            _ => Self::Simple,
        })
    }
}

/// A software authenticator for the tests here and the sign-in page's.
#[cfg(test)]
pub(super) mod fake {
    use super::*;
    use aws_lc_rs::rand::SystemRandom;
    use aws_lc_rs::signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair};

    /// A minimal CBOR writer for building what an authenticator sends.
    fn head(major: u8, value: u64, out: &mut Vec<u8>) {
        match value {
            0..=23 => out.push((major << 5) | value as u8),
            24..=0xff => out.extend([(major << 5) | 24, value as u8]),
            _ => {
                out.push((major << 5) | 25);
                out.extend((value as u16).to_be_bytes());
            }
        }
    }

    fn int(value: i64, out: &mut Vec<u8>) {
        if value >= 0 {
            head(0, value as u64, out);
        } else {
            head(1, (-1 - value) as u64, out);
        }
    }

    fn bytes(value: &[u8], out: &mut Vec<u8>) {
        head(2, value.len() as u64, out);
        out.extend_from_slice(value);
    }

    fn text(value: &str, out: &mut Vec<u8>) {
        head(3, value.len() as u64, out);
        out.extend_from_slice(value.as_bytes());
    }

    pub(crate) struct Authenticator {
        key: EcdsaKeyPair,
        id: Vec<u8>,
        pub(crate) count: u32,
    }

    impl Authenticator {
        pub(crate) fn new() -> Self {
            let rng = SystemRandom::new();
            let pkcs8 =
                EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
            Self {
                key: EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref())
                    .unwrap(),
                id: vec![7; 16],
                count: 0,
            }
        }

        fn auth_data(&self, rp: &str, attested: bool) -> Vec<u8> {
            let mut data = Sha256::digest(rp.as_bytes()).to_vec();
            data.push(
                FLAG_USER_PRESENT | FLAG_USER_VERIFIED | if attested { FLAG_ATTESTED } else { 0 },
            );
            data.extend(self.count.to_be_bytes());
            if attested {
                data.extend([0u8; 16]);
                data.extend((self.id.len() as u16).to_be_bytes());
                data.extend(&self.id);
                let point = self.key.public_key().as_ref();
                let mut cose = Vec::new();
                head(5, 5, &mut cose);
                int(1, &mut cose);
                int(2, &mut cose);
                int(3, &mut cose);
                int(ALG_ES256, &mut cose);
                int(-1, &mut cose);
                int(1, &mut cose);
                int(-2, &mut cose);
                bytes(&point[1..33], &mut cose);
                int(-3, &mut cose);
                bytes(&point[33..65], &mut cose);
                data.extend(cose);
            }
            data
        }

        fn client(ceremony: &str, challenge: &str, origin: &str) -> Vec<u8> {
            json!({"type": ceremony, "challenge": challenge, "origin": origin, "crossOrigin": false})
                .to_string()
                .into_bytes()
        }

        pub(crate) fn create(&self, challenge: &str, origin: &str) -> (String, String) {
            let mut object = Vec::new();
            head(5, 3, &mut object);
            text("fmt", &mut object);
            text("none", &mut object);
            text("attStmt", &mut object);
            head(5, 0, &mut object);
            text("authData", &mut object);
            bytes(&self.auth_data(&rp_id(origin), true), &mut object);
            (
                URL_SAFE_NO_PAD.encode(Self::client("webauthn.create", challenge, origin)),
                URL_SAFE_NO_PAD.encode(object),
            )
        }

        pub(crate) fn get(&mut self, challenge: &str, origin: &str) -> [String; 4] {
            self.count += 1;
            let client = Self::client("webauthn.get", challenge, origin);
            let data = self.auth_data(&rp_id(origin), false);
            let mut message = data.clone();
            message.extend_from_slice(&Sha256::digest(&client));
            let signature = self.key.sign(&SystemRandom::new(), &message).unwrap();
            [
                URL_SAFE_NO_PAD.encode(&self.id),
                URL_SAFE_NO_PAD.encode(client),
                URL_SAFE_NO_PAD.encode(data),
                URL_SAFE_NO_PAD.encode(signature.as_ref()),
            ]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Authenticator;
    use super::*;

    const ORIGIN: &str = "https://rustcode.example.com";

    fn paths(dir: &Path) -> (PathBuf, PathBuf) {
        (dir.join(PASSKEY_FILE), dir.join(ENROLLMENT_FILE))
    }

    fn enroll(dir: &Path, authenticator: &Authenticator) -> Passkey {
        let (passkeys, enrollments) = paths(dir);
        let ticket = create_enrollment_at(&enrollments, &passkeys, "Phone").unwrap();
        let offer = enrollment_at(&enrollments, &ticket).expect("the ticket is open");
        let (client_data, attestation) = authenticator.create(&offer.challenge, ORIGIN);
        register_at(
            &passkeys,
            &enrollments,
            ORIGIN,
            &Registration {
                ticket: &ticket,
                client_data: &client_data,
                attestation: &attestation,
            },
        )
        .unwrap()
    }

    fn assert_with(dir: &Path, answer: &[String; 4], origin: &str) -> Result<String, String> {
        verify_assertion_at(
            &paths(dir).0,
            origin,
            &Assertion {
                id: &answer[0],
                client_data: &answer[1],
                authenticator_data: &answer[2],
                signature: &answer[3],
            },
        )
    }

    #[test]
    fn a_passkey_is_added_once_from_a_ticket_and_approves_a_sign_in() {
        let dir = tempfile::tempdir().unwrap();
        let mut phone = Authenticator::new();
        let passkey = enroll(dir.path(), &phone);
        assert_eq!(passkey.name, "Phone");
        assert_eq!(passkey.rp_id, "rustcode.example.com");
        assert_eq!(list_at(&paths(dir.path()).0).len(), 1);
        let answer = phone.get(&new_challenge(), ORIGIN);
        assert_eq!(assert_with(dir.path(), &answer, ORIGIN).unwrap(), "Phone");
        assert!(list_at(&paths(dir.path()).0)[0].last_used_at.is_some());
    }

    #[test]
    fn an_enrollment_ticket_works_once() {
        let dir = tempfile::tempdir().unwrap();
        let (passkeys, enrollments) = paths(dir.path());
        let ticket = create_enrollment_at(&enrollments, &passkeys, "Phone").unwrap();
        let offer = enrollment_at(&enrollments, &ticket).unwrap();
        let phone = Authenticator::new();
        let (client_data, attestation) = phone.create(&offer.challenge, ORIGIN);
        let posted = Registration {
            ticket: &ticket,
            client_data: &client_data,
            attestation: &attestation,
        };
        register_at(&passkeys, &enrollments, ORIGIN, &posted).unwrap();
        let again = register_at(&passkeys, &enrollments, ORIGIN, &posted).unwrap_err();
        assert!(again.contains("used already"), "{again}");
        assert!(enrollment_at(&enrollments, "not-a-ticket").is_none());
    }

    #[test]
    fn an_assertion_from_another_origin_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut phone = Authenticator::new();
        enroll(dir.path(), &phone);
        // The browser reports where the page really was.
        let answer = phone.get(&new_challenge(), "https://evil.example.net");
        let refused = assert_with(dir.path(), &answer, ORIGIN).unwrap_err();
        assert!(refused.contains("different site"), "{refused}");
        // Signed for this origin, but for a passkey of another host.
        let answer = phone.get(&new_challenge(), "https://other.example.com");
        let refused = assert_with(dir.path(), &answer, "https://other.example.com").unwrap_err();
        assert!(refused.contains("not one of"), "{refused}");
    }

    #[test]
    fn a_replayed_assertion_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut phone = Authenticator::new();
        enroll(dir.path(), &phone);
        let answer = phone.get(&new_challenge(), ORIGIN);
        assert!(assert_with(dir.path(), &answer, ORIGIN).is_ok());
        let replayed = assert_with(dir.path(), &answer, ORIGIN).unwrap_err();
        assert!(replayed.contains("used already"), "{replayed}");
        // A fresh challenge with a counter that did not move is a copy.
        phone.count -= 1;
        let copied = phone.get(&new_challenge(), ORIGIN);
        let refused = assert_with(dir.path(), &copied, ORIGIN).unwrap_err();
        assert!(refused.contains("counter"), "{refused}");
    }

    #[test]
    fn a_challenge_nobody_issued_and_a_bad_signature_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let mut phone = Authenticator::new();
        enroll(dir.path(), &phone);
        let answer = phone.get("made-up", ORIGIN);
        assert!(assert_with(dir.path(), &answer, ORIGIN).is_err());
        let mut answer = phone.get(&new_challenge(), ORIGIN);
        answer[3] = URL_SAFE_NO_PAD.encode([0u8; 70]);
        let refused = assert_with(dir.path(), &answer, ORIGIN).unwrap_err();
        assert!(refused.contains("signature"), "{refused}");
    }

    #[test]
    fn what_the_scripts_embed_cannot_end_the_script() {
        assert_eq!(
            js("</script><b>&"),
            r#""\u003c/script\u003e\u003cb\u003e\u0026""#
        );
    }

    #[test]
    fn a_passkey_is_removed_by_id_prefix_or_name() {
        let dir = tempfile::tempdir().unwrap();
        let phone = Authenticator::new();
        let passkey = enroll(dir.path(), &phone);
        let path = paths(dir.path()).0;
        assert!(remove_at(&path, "nothing").is_err());
        assert_eq!(remove_at(&path, "phone").unwrap().id, passkey.id);
        assert!(list_at(&path).is_empty());
    }

    #[test]
    fn malformed_cbor_is_refused_without_panicking() {
        for input in [
            &[][..],
            &[0x5f],
            &[0x59, 0xff, 0xff],
            &[0xa1, 0x01],
            &[0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        ] {
            assert!(Cbor::decode(input).is_err(), "{input:?}");
        }
        let mut deep = vec![0x81; MAX_DEPTH + 2];
        deep.push(0x00);
        assert!(Cbor::decode(&deep).is_err());
    }
}
