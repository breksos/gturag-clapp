//! The launch bootstrap (reference/launch.md): [`clatch_init`], the
//! `SteamAPI_RestartAppIfNecessary` equivalent. An app calls it first thing in
//! `main`; it guarantees the app runs only under Clatch by relaunching itself
//! through the launcher when it was started any other way. It is env checks plus
//! one `clatch run`, so any language can implement it; this is the Rust convenience.

use crate::identity::{ENV_APP_ID, ENV_TOKEN};
use clatch_core::{ClatchError, Result};
use std::process::Command;

/// `CLATCH_STANDALONE=1` opts out of the relaunch (the dev hatch): [`clatch_init`]
/// returns `false` so the app keeps running without a launcher.
pub const ENV_STANDALONE: &str = "CLATCH_STANDALONE";
/// `CLATCH_BIN` overrides the `clatch` binary used for the relaunch (tests, custom
/// installs); otherwise `clatch` is resolved on `PATH`. The name itself is the
/// shared [`clatch_core::ENV_BIN`]; this re-export keeps the pipe crate's API.
pub const ENV_BIN: &str = clatch_core::ENV_BIN;

/// Ensure this process runs under Clatch (reference/launch.md), returning whether it
/// handed off. **`Ok(true)` means the caller must exit now**, Clatch is relaunching
/// the installed copy.
///
/// - **Wired** (`CLATCH_INSTANCE_TOKEN` present): Clatch spawned us. Returns
///   `Ok(false)`; connect and register next ([`crate::Client`]). A `CLATCH_APP_ID`
///   that disagrees with `app_id` is a hard error (wrong binary or manifest id).
/// - **Standalone** (`CLATCH_STANDALONE=1`): the dev hatch. Returns `Ok(false)`.
/// - **Otherwise**: asks Clatch to launch the *installed* copy (`clatch run
///   <app_id>`, which starts the daemon if needed), then returns `Ok(true)`. An
///   error means Clatch could not take over (not installed, `clatch` not found); the
///   app must not run bare.
pub fn clatch_init(app_id: &str) -> Result<bool> {
    if std::env::var_os(ENV_TOKEN).is_some() {
        // Clatch injects identity before our code runs, so a relaunched copy always
        // lands here: this is the loop guard.
        if let Some(wired) = std::env::var_os(ENV_APP_ID) {
            if wired != *app_id {
                return Err(ClatchError::Invalid(format!(
                    "launched as {:?} but this binary is {app_id}",
                    wired.to_string_lossy()
                )));
            }
        }
        return Ok(false);
    }
    if std::env::var_os(ENV_STANDALONE).is_some() {
        return Ok(false);
    }
    relaunch(app_id).map(|()| true)
}

/// Hand off to the launcher: `clatch run <app_id>` (the `clatch://run/<app_id>`
/// path) starts the daemon if needed and launches the installed copy. We wait for
/// the launch to be acknowledged, so a failure surfaces to the caller.
fn relaunch(app_id: &str) -> Result<()> {
    let bin = std::env::var_os(ENV_BIN).unwrap_or_else(|| "clatch".into());
    let mut cmd = Command::new(&bin);
    cmd.arg("run").arg(app_id);
    // No console flash when an app relaunches itself managed (Windows): the same
    // rule the GUI and daemon spawns already follow.
    clatch_core::process::hide_console(&mut cmd);
    let status = cmd.status().map_err(|e| ClatchError::io(&bin, e))?;
    if !status.success() {
        return Err(ClatchError::Invalid(format!(
            "clatch could not launch {app_id} (is it installed?)"
        )));
    }
    Ok(())
}
