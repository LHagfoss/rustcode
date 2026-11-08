//! Who may connect: the user the server runs as, and nobody else.
//!
//! On Unix the socket is `0600` in a `0700` directory, and each connection's
//! peer credentials are checked as well, so a socket reached some other way
//! (a loosened directory, a passed descriptor) still refuses another user.
//! On Windows the pipe's DACL grants access to its owner and SYSTEM only
//! (`listen.rs`), and its name is random and published in the user's own
//! profile, so another user can neither open it nor squat on it.

/// Whether a peer running as `peer` may use a server running as `server`.
/// Root gets no exception: it can become the user if it must.
#[cfg(any(unix, test))]
pub(crate) fn allowed(peer: u32, server: u32) -> bool {
    peer == server
}

/// Refuse a connection from another user.
#[cfg(unix)]
pub(crate) fn check(stream: &tokio::net::UnixStream) -> std::io::Result<()> {
    let peer = stream.peer_cred()?.uid();
    // SAFETY: geteuid has no preconditions and cannot fail.
    let server = unsafe { libc::geteuid() };
    if allowed(peer, server) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("a process of uid {peer} may not use the server of uid {server}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_same_user_is_allowed() {
        assert!(allowed(501, 501));
        assert!(!allowed(502, 501));
        assert!(!allowed(0, 501), "root is another user");
        assert!(!allowed(501, 0));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_connection_from_this_user_passes() {
        let (server, _client) = tokio::net::UnixStream::pair().expect("pair");
        check(&server).expect("this user may connect");
    }
}
