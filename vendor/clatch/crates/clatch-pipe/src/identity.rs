//! Identity by injection (reference/protocol.md, section Launch and Transport).
//! Clatch is the parent: it mints a per-run identity, applies it to the spawned
//! app's environment, and the app reads it back. The token proves "this process
//! is the one Clatch spawned"; the wire only proves what the parent assigned.

use clatch_core::AppId;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

/// Injected environment variable names (reference/protocol.md § Transport). No
/// protocol version is injected: the major an app targets is its manifest's
/// `protocol`, validated at install (reference/protocol.md § Versioning).
pub const ENV_APP_ID: &str = "CLATCH_APP_ID";
pub const ENV_INSTANCE_ID: &str = "CLATCH_INSTANCE_ID";
pub const ENV_CONTROL_ADDR: &str = "CLATCH_CONTROL_ADDR";
pub const ENV_TOKEN: &str = "CLATCH_INSTANCE_TOKEN";

/// A per-run identity Clatch assigns to a spawned app: who it is, which run, where
/// to connect back, and a one-time secret. Minted launcher-side, injected into the
/// child env, proved back over the pipe at register.
#[derive(Debug, Clone)]
pub struct Identity {
    pub app_id: AppId,
    pub instance_id: String,
    pub token: String,
    /// Socket path (unix) or pipe name (windows) the app connects back to.
    pub addr: String,
}

impl Identity {
    /// Mint a fresh identity for `app` whose control socket lives under `run_dir`
    /// (`~/.clatch/run`). Instance id and token are random.
    pub fn mint(app: AppId, run_dir: &Path) -> Self {
        let instance_id = rand_hex(8); // 16 hex chars
        let token = rand_hex(32); // 64 hex chars, the secret
        let addr = control_addr(run_dir, &instance_id);
        Self {
            app_id: app,
            instance_id,
            token,
            addr,
        }
    }

    /// Apply the identity to a child process's environment, before its code runs.
    pub fn inject(&self, cmd: &mut Command) {
        cmd.env(ENV_APP_ID, self.app_id.as_str())
            .env(ENV_INSTANCE_ID, &self.instance_id)
            .env(ENV_CONTROL_ADDR, &self.addr)
            .env(ENV_TOKEN, &self.token);
    }

    /// Read the injected identity back, app-side (the reference binding and the
    /// dev escape hatch). `None` if this process was not launched by Clatch.
    pub fn from_env() -> Option<Self> {
        Some(Self {
            app_id: AppId::new(std::env::var(ENV_APP_ID).ok()?),
            instance_id: std::env::var(ENV_INSTANCE_ID).ok()?,
            token: std::env::var(ENV_TOKEN).ok()?,
            addr: std::env::var(ENV_CONTROL_ADDR).ok()?,
        })
    }
}

/// The control address for an instance: a unix socket under `run_dir`, or a
/// Windows named pipe (reference/protocol.md, section Transport).
#[cfg(unix)]
fn control_addr(run_dir: &Path, instance_id: &str) -> String {
    run_dir
        .join(format!("{instance_id}.sock"))
        .to_string_lossy()
        .into_owned()
}

#[cfg(windows)]
fn control_addr(_run_dir: &Path, instance_id: &str) -> String {
    format!("{}{instance_id}", clatch_core::PIPE_PREFIX)
}

/// `2 * bytes` hex chars from the OS RNG.
fn rand_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("OS RNG");
    let mut s = String::with_capacity(bytes * 2);
    for b in buf {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn mint_is_unique_and_addressed() {
        let dir = PathBuf::from("/tmp/clatch-run");
        let a = Identity::mint(AppId::new("com.arfium.testapp"), &dir);
        let b = Identity::mint(AppId::new("com.arfium.testapp"), &dir);
        assert_ne!(a.instance_id, b.instance_id, "instance ids are unique");
        assert_ne!(a.token, b.token, "tokens are unique");
        assert_eq!(a.token.len(), 64, "token is 32 random bytes, hex");
        assert!(
            a.addr.contains(&a.instance_id),
            "addr carries the instance id"
        );
    }
}
