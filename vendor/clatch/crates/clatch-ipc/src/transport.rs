//! Transport: a local control socket, a unix domain socket on Linux/macOS, a named
//! pipe on Windows. The owner binds the address and accepts connections; the dialer
//! connects. The socket directory is created `0700`. Everything above this module
//! is platform-agnostic and works over any `AsyncRead + AsyncWrite` the listener or
//! connector yields.

use std::io;

#[cfg(unix)]
pub use unix::{connect, Listener};

#[cfg(windows)]
pub use windows::{connect, Listener};

#[cfg(unix)]
mod unix {
    use super::io;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tokio::net::{UnixListener, UnixStream};

    /// A bound listener. The owner binds an address and accepts the peers that
    /// connect back.
    pub struct Listener {
        inner: UnixListener,
        addr: String,
    }

    impl Listener {
        /// Bind at `addr`, creating the parent dir `0700`. Fails with `AddrInUse`
        /// if the address is already taken; the caller decides whether that is a
        /// live peer or a stale file to reclaim. (Per-instance app sockets use a
        /// fresh random path, so they never collide.)
        pub fn bind(addr: &str) -> io::Result<Self> {
            if let Some(dir) = Path::new(addr).parent() {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            let inner = UnixListener::bind(addr)?;
            Ok(Self {
                inner,
                addr: addr.into(),
            })
        }

        pub fn addr(&self) -> &str {
            &self.addr
        }

        /// Accept the next connection.
        pub async fn accept(&self) -> io::Result<UnixStream> {
            let (stream, _) = self.inner.accept().await?;
            Ok(stream)
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.addr);
        }
    }

    /// Connect to a listener at `addr`.
    pub async fn connect(addr: &str) -> io::Result<UnixStream> {
        UnixStream::connect(addr).await
    }
}

#[cfg(windows)]
mod windows {
    use super::io;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    /// All server instances busy: the accept-window (B6). Retried. A missing
    /// name (`ERROR_FILE_NOT_FOUND`) is NOT retried: while a server lives, its
    /// name always has at least one instance (the connected one counts), so an
    /// absent name means no server, and stalling on it would tax every cold
    /// `clatch` start with a five-second wait for a daemon that is not there.
    /// The boot race (client dials while the daemon binds) is the caller's:
    /// `bootstrap::connect_or_start` already polls with its own deadline.
    const ERROR_PIPE_BUSY: i32 = 231;

    /// A per-instance named-pipe listener. Compile-checked on Windows only; the
    /// platform-agnostic layers above are exercised on unix.
    pub struct Listener {
        addr: String,
        first: Mutex<Option<NamedPipeServer>>,
    }

    impl Listener {
        pub fn bind(addr: &str) -> io::Result<Self> {
            // NO first-instance reservation: a pipe NAME lives while any handle
            // to any of its instances does, including a dead server's still-
            // connected clients, so a reserved name blocks a legitimate rebind
            // (a daemon restart under a lingering GUI watcher). Ownership is not
            // the transport's job: the daemon's singleton is a lock file
            // (clatch-daemon::singleton), and binding here is pure transport.
            let first = ServerOptions::new().create(addr)?;
            Ok(Self {
                addr: addr.into(),
                first: Mutex::new(Some(first)),
            })
        }

        pub fn addr(&self) -> &str {
            &self.addr
        }

        pub async fn accept(&self) -> io::Result<NamedPipeServer> {
            // Use the reserved first instance, then make a fresh one per accept;
            // the guard is dropped before the await.
            let server = match self.first.lock().unwrap().take() {
                Some(s) => s,
                None => ServerOptions::new().create(&self.addr)?,
            };
            server.connect().await?;
            Ok(server)
        }
    }

    /// Dial the pipe, retrying the accept-window (B6): between the server's
    /// accepts every instance can momentarily be busy (`ERROR_PIPE_BUSY`), and a
    /// single `open` would then fail a perfectly valid dial. Bounded (~5s) so a
    /// name whose server died with clients still attached (busy corpse
    /// instances, nobody listening) errors instead of spinning forever. An
    /// absent name errors immediately: no server, the caller's business.
    ///
    /// Windows-only: this module is not compiled or run on the unix dev host, so it
    /// is verified on a Windows build, not by the macOS gate.
    pub async fn connect(addr: &str) -> io::Result<NamedPipeClient> {
        for _ in 0..250 {
            match ClientOptions::new().open(addr) {
                Ok(client) => return Ok(client),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        }
        // Bound exhausted: one last try to surface the real, persistent error.
        ClientOptions::new().open(addr)
    }
}
