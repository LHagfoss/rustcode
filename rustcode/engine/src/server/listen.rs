//! The server's local endpoint: a Unix domain socket on macOS and Linux, a
//! named pipe on Windows. Both carry newline-delimited ACP JSON-RPC.

use std::io;
use std::path::Path;

#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(unix)]
mod unix {
    use super::*;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};

    /// The server's side of a connection.
    pub(crate) type Stream = UnixStream;
    /// A client's side.
    pub(crate) type ClientStream = UnixStream;

    /// Where the server in `run_dir` listens.
    pub(crate) fn endpoint(run_dir: &Path) -> String {
        run_dir.join("server.sock").to_string_lossy().into_owned()
    }

    /// The socket, removed when dropped.
    pub(crate) struct Listener {
        listener: UnixListener,
        path: PathBuf,
    }

    /// Unix has no bind-with-mode API. Apply a restrictive umask only while
    /// the socket inode is created, then restore the process setting.
    struct SocketUmask(libc::mode_t);

    impl SocketUmask {
        fn private() -> Self {
            // SAFETY: umask has no preconditions; Drop restores the old mask.
            Self(unsafe { libc::umask(0o177) })
        }
    }

    impl Drop for SocketUmask {
        fn drop(&mut self) {
            // SAFETY: this restores the mask captured by `private`.
            unsafe {
                libc::umask(self.0);
            }
        }
    }

    impl Listener {
        /// Listen on `endpoint`. The caller holds the server lock, so a
        /// socket already there is a dead server's and is replaced.
        pub(crate) fn bind(endpoint: &str) -> io::Result<Self> {
            use std::os::unix::fs::PermissionsExt;
            let path = PathBuf::from(endpoint);
            match std::fs::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
            let listener = {
                let _umask = SocketUmask::private();
                UnixListener::bind(&path)
            }?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            Ok(Self { listener, path })
        }

        /// The next client of this user. Another user's connection is
        /// refused and closed here.
        pub(crate) async fn accept(&mut self) -> io::Result<Stream> {
            let (stream, _) = self.listener.accept().await?;
            super::super::peer::check(&stream)?;
            Ok(stream)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Connect to the server listening on `endpoint`.
    pub(crate) async fn connect(endpoint: &str) -> io::Result<ClientStream> {
        UnixStream::connect(endpoint).await
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    /// The server's side of a connection.
    pub(crate) type Stream = NamedPipeServer;
    /// A client's side.
    pub(crate) type ClientStream = NamedPipeClient;

    /// A pipe name no other user can guess: it is published only in this
    /// user's profile (`server.json`).
    pub(crate) fn endpoint(_run_dir: &Path) -> String {
        format!(
            r"\\.\pipe\rustcode-server-{}",
            uuid::Uuid::new_v4().simple()
        )
    }

    /// The pipe, with the next instance waiting for a client.
    pub(crate) struct Listener {
        name: String,
        next: NamedPipeServer,
        security: security::OwnerOnly,
    }

    impl Listener {
        pub(crate) fn bind(endpoint: &str) -> io::Result<Self> {
            let mut security = security::OwnerOnly::new()?;
            // The first instance fails if the name is taken, by anyone.
            let next = create(endpoint, &mut security, true)?;
            Ok(Self {
                name: endpoint.to_owned(),
                next,
                security,
            })
        }

        pub(crate) async fn accept(&mut self) -> io::Result<Stream> {
            self.next.connect().await?;
            let next = create(&self.name, &mut self.security, false)?;
            Ok(std::mem::replace(&mut self.next, next))
        }
    }

    fn create(
        name: &str,
        security: &mut security::OwnerOnly,
        first: bool,
    ) -> io::Result<NamedPipeServer> {
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        // SAFETY: the attributes and the descriptor they point to live in
        // `security`, which outlives this call; the pipe copies them.
        unsafe { options.create_with_security_attributes_raw(name, security.as_ptr()) }
    }

    /// Connect to the server listening on `endpoint`, waiting while every
    /// instance is busy.
    pub(crate) async fn connect(endpoint: &str) -> io::Result<ClientStream> {
        const ERROR_PIPE_BUSY: i32 = 231;
        loop {
            match ClientOptions::new().open(endpoint) {
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                other => return other,
            }
        }
    }

    /// A security descriptor that lets the pipe's owner (this user) and
    /// SYSTEM in, and nobody else. Declared here rather than through a
    /// Windows bindings crate: it is two calls.
    mod security {
        use std::ffi::c_void;
        use std::io;

        #[repr(C)]
        struct SecurityAttributes {
            length: u32,
            descriptor: *mut c_void,
            inherit_handle: i32,
        }

        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
                descriptor: *const u16,
                revision: u32,
                security_descriptor: *mut *mut c_void,
                size: *mut u32,
            ) -> i32;
        }

        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn LocalFree(memory: *mut c_void) -> *mut c_void;
        }

        const SDDL_REVISION_1: u32 = 1;
        /// Protected DACL: full access for the owner (OW) and SYSTEM (SY).
        const OWNER_ONLY: &str = "D:P(A;;GA;;;OW)(A;;GA;;;SY)";

        pub(crate) struct OwnerOnly {
            attributes: Box<SecurityAttributes>,
        }

        // The descriptor is only read by the system, and freed once.
        unsafe impl Send for OwnerOnly {}
        unsafe impl Sync for OwnerOnly {}

        impl OwnerOnly {
            pub(crate) fn new() -> io::Result<Self> {
                let sddl = OWNER_ONLY
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect::<Vec<_>>();
                let mut descriptor = std::ptr::null_mut();
                // SAFETY: `sddl` is NUL-terminated UTF-16; the system
                // allocates the descriptor, freed in `drop`.
                let converted = unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        sddl.as_ptr(),
                        SDDL_REVISION_1,
                        &mut descriptor,
                        std::ptr::null_mut(),
                    )
                };
                if converted == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self {
                    attributes: Box::new(SecurityAttributes {
                        length: std::mem::size_of::<SecurityAttributes>() as u32,
                        descriptor,
                        inherit_handle: 0,
                    }),
                })
            }

            pub(crate) fn as_ptr(&mut self) -> *mut c_void {
                (&mut *self.attributes as *mut SecurityAttributes).cast()
            }
        }

        impl Drop for OwnerOnly {
            fn drop(&mut self) {
                // SAFETY: allocated by the conversion above, freed once.
                unsafe {
                    LocalFree(self.attributes.descriptor);
                }
            }
        }
    }
}

/// Endpoint published by the server in this gateway's configuration directory.
pub(crate) fn running_endpoint(config_directory: &Path) -> Option<String> {
    super::lock::read_info(&config_directory.join("run")).map(|info| info.endpoint)
}
