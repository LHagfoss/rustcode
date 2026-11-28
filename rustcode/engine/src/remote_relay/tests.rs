use super::pin::{pinned_connector, server_name};
use super::protocol::{ErrorCode, Reply, Request, Role, read_frame, signed_message, write_frame};
use super::server::{Limits, Relay};
use super::state::RelayState;
use crate::remote_gateway::tls::TlsIdentity;
use aws_lc_rs::signature::{Ed25519KeyPair, KeyPair as _};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::client::TlsStream;

struct Key(Ed25519KeyPair);

impl Key {
    fn new() -> Self {
        Self(Ed25519KeyPair::generate().expect("generate an Ed25519 key"))
    }

    fn public(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.0.public_key().as_ref())
    }

    fn sign(&self, role: Role, relay_id: &str, challenge: &str, machine_id: &str) -> String {
        let message = signed_message(role, relay_id, challenge, machine_id);
        URL_SAFE_NO_PAD.encode(self.0.sign(&message).as_ref())
    }
}

type Control = (
    BufReader<ReadHalf<TlsStream<TcpStream>>>,
    WriteHalf<TlsStream<TcpStream>>,
);

fn test_limits() -> Limits {
    Limits {
        handshake_timeout: Duration::from_secs(5),
        pipe_wait: Duration::from_millis(300),
        ..Limits::default()
    }
}

async fn start(limits: Limits) -> (Arc<Relay>, SocketAddr) {
    let relay = Relay::new(
        RelayState::new(),
        None,
        TlsIdentity::ephemeral().unwrap(),
        limits,
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(Arc::clone(&relay).run(listener));
    (relay, address)
}

async fn register(
    relay: &Relay,
    address: SocketAddr,
    machine_id: &str,
    key: &Key,
    secret: &str,
) -> Result<Control, Reply> {
    let tcp = TcpStream::connect(address).await.unwrap();
    let tls = pinned_connector(relay.fingerprint())
        .unwrap()
        .connect(server_name(), tcp)
        .await
        .unwrap();
    let (read, mut write) = tokio::io::split(tls);
    let mut read = BufReader::new(read);
    let register = Request::Register {
        version: 1,
        secret: secret.to_owned(),
        machine_id: machine_id.to_owned(),
        public_key: key.public(),
    };
    write_frame(&mut write, &register).await.unwrap();
    let reply: Reply = read_frame(&mut read).await.unwrap().unwrap();
    let Reply::Challenge { challenge, .. } = reply else {
        return Err(reply);
    };
    let prove = Request::Prove {
        signature: key.sign(Role::Member, &relay.relay_id(), &challenge, machine_id),
    };
    write_frame(&mut write, &prove).await.unwrap();
    match read_frame(&mut read).await.unwrap().unwrap() {
        Reply::Registered { .. } => Ok((read, write)),
        other => Err(other),
    }
}

async fn allow(relay: &Relay, control: &mut Control, machine_id: &str, key: &Key) {
    let keys = Request::AllowedKeys {
        keys: vec![key.public()],
    };
    write_frame(&mut control.1, &keys).await.unwrap();
    for _ in 0..200 {
        if relay.allows(machine_id, &key.public()) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the relay never took the allowed keys");
}

async fn dial(
    relay: &Relay,
    address: SocketAddr,
    machine_id: &str,
    key: &Key,
    sign_with: &Key,
) -> Result<BufReader<TcpStream>, Reply> {
    let mut stream = BufReader::new(TcpStream::connect(address).await.unwrap());
    let dial = Request::Dial {
        version: 1,
        machine_id: machine_id.to_owned(),
        public_key: key.public(),
    };
    write_frame(stream.get_mut(), &dial).await.unwrap();
    let reply: Reply = read_frame(&mut stream).await.unwrap().unwrap();
    let Reply::Challenge { challenge, .. } = reply else {
        return Err(reply);
    };
    let prove = Request::Prove {
        signature: sign_with.sign(Role::Client, &relay.relay_id(), &challenge, machine_id),
    };
    write_frame(stream.get_mut(), &prove).await.unwrap();
    match read_frame(&mut stream).await.unwrap().unwrap() {
        Reply::Connected => Ok(stream),
        other => Err(other),
    }
}

fn code(reply: &Reply) -> Option<ErrorCode> {
    match reply {
        Reply::Error { code, .. } => Some(*code),
        _ => None,
    }
}

#[tokio::test]
async fn a_paired_device_reaches_its_machine_through_a_blind_pipe() {
    let (relay, address) = start(test_limits()).await;
    let machine_key = Key::new();
    let device_key = Key::new();
    let mut control = register(&relay, address, "mac", &machine_key, &relay.secret())
        .await
        .unwrap();
    allow(&relay, &mut control, "mac", &device_key).await;

    let dialing = {
        let relay = Arc::clone(&relay);
        tokio::spawn(async move { dial(&relay, address, "mac", &device_key, &device_key).await })
    };
    let Reply::Open { pipe } = read_frame(&mut control.0).await.unwrap().unwrap() else {
        panic!("the member was not asked for a pipe");
    };
    let mut member_pipe = TcpStream::connect(address).await.unwrap();
    write_frame(&mut member_pipe, &Request::Pipe { pipe })
        .await
        .unwrap();
    let mut device = dialing.await.unwrap().unwrap();

    device.get_mut().write_all(b"\x16hello").await.unwrap();
    let mut heard = [0u8; 6];
    member_pipe.read_exact(&mut heard).await.unwrap();
    assert_eq!(&heard, b"\x16hello");

    member_pipe.write_all(b"world").await.unwrap();
    let mut answer = [0u8; 5];
    device.read_exact(&mut answer).await.unwrap();
    assert_eq!(&answer, b"world");
}

#[tokio::test]
async fn a_wrong_secret_registers_nothing() {
    let (relay, address) = start(test_limits()).await;
    let reply = register(&relay, address, "mac", &Key::new(), "not-the-secret")
        .await
        .err()
        .unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::Unauthorized));
    assert_eq!(relay.online(), 0);
}

#[tokio::test]
async fn a_machine_id_stays_bound_to_its_first_key() {
    let (relay, address) = start(test_limits()).await;
    let first = Key::new();
    drop(
        register(&relay, address, "mac", &first, &relay.secret())
            .await
            .unwrap(),
    );
    let reply = register(&relay, address, "mac", &Key::new(), &relay.secret())
        .await
        .err()
        .unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::KeyMismatch));
    assert!(
        register(&relay, address, "mac", &first, &relay.secret())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_dial_to_a_machine_that_is_not_registered_is_refused() {
    let (relay, address) = start(test_limits()).await;
    let key = Key::new();
    let reply = dial(&relay, address, "nowhere", &key, &key)
        .await
        .err()
        .unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::MachineOffline));
}

#[tokio::test]
async fn a_device_the_machine_does_not_allow_gets_no_pipe() {
    let (relay, address) = start(test_limits()).await;
    let mut control = register(&relay, address, "mac", &Key::new(), &relay.secret())
        .await
        .unwrap();
    allow(&relay, &mut control, "mac", &Key::new()).await;
    let stranger = Key::new();
    let reply = dial(&relay, address, "mac", &stranger, &stranger)
        .await
        .err()
        .unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::NotAllowed));
}

#[tokio::test]
async fn a_signature_by_another_key_is_refused() {
    let (relay, address) = start(test_limits()).await;
    let device = Key::new();
    let mut control = register(&relay, address, "mac", &Key::new(), &relay.secret())
        .await
        .unwrap();
    allow(&relay, &mut control, "mac", &device).await;
    let reply = dial(&relay, address, "mac", &device, &Key::new())
        .await
        .err()
        .unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::Unauthorized));
}

#[tokio::test]
async fn a_member_that_opens_no_pipe_times_the_dial_out() {
    let (relay, address) = start(test_limits()).await;
    let device = Key::new();
    let mut control = register(&relay, address, "mac", &Key::new(), &relay.secret())
        .await
        .unwrap();
    allow(&relay, &mut control, "mac", &device).await;
    let reply = dial(&relay, address, "mac", &device, &device)
        .await
        .err()
        .unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::Timeout));
}

#[tokio::test]
async fn a_registration_in_the_clear_is_refused() {
    let (relay, address) = start(test_limits()).await;
    let mut stream = BufReader::new(TcpStream::connect(address).await.unwrap());
    let register = Request::Register {
        version: 1,
        secret: relay.secret(),
        machine_id: "mac".to_owned(),
        public_key: Key::new().public(),
    };
    write_frame(stream.get_mut(), &register).await.unwrap();
    let reply: Reply = read_frame(&mut stream).await.unwrap().unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::BadRequest));
    assert_eq!(relay.online(), 0);
}

#[tokio::test]
async fn a_registration_from_another_protocol_version_is_refused() {
    let (relay, address) = start(test_limits()).await;
    let tcp = TcpStream::connect(address).await.unwrap();
    let tls = pinned_connector(relay.fingerprint())
        .unwrap()
        .connect(server_name(), tcp)
        .await
        .unwrap();
    let (read, mut write) = tokio::io::split(tls);
    let mut read = BufReader::new(read);
    let register = Request::Register {
        version: 99,
        secret: relay.secret(),
        machine_id: "mac".to_owned(),
        public_key: Key::new().public(),
    };
    write_frame(&mut write, &register).await.unwrap();
    let reply: Reply = read_frame(&mut read).await.unwrap().unwrap();
    assert_eq!(code(&reply), Some(ErrorCode::UnsupportedVersion));
}
