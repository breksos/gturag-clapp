//! One binary, two roles over one state — and one engine, rebranded per registry.
//!
//! `gturag app` is the window Clatch launches; `gturag <verb>` is the agent's CLI.
//! [`clappkit::role::main_dispatch`] decides which at startup, which is why this file is
//! six lines and every clapp's is the same six.
//!
//! Deliberately NO `#![windows_subsystem = "windows"]`: the attribute applies to the whole
//! image, but this image is two roles. A GUI-subsystem process gets no console and is not
//! waited on by the `.cmd` shim Clatch links onto the agent's PATH, so every CLI call would
//! return instantly, empty, with exit code 0. Clatch already spawns the launch command with
//! `CREATE_NO_WINDOW`, so a console-subsystem clapp shows no console anyway.

mod app;
mod build_index;
mod cli;
mod corpus;
mod embed;
mod index;
mod provision;
mod state;

/// The app's identity, read from `clatch.json` by `build.rs`. Not constants: a fork of
/// this engine changes the manifest and rebuilds, and nothing here has to know.
const APP_ID: &str = env!("CLAPP_ID");
/// The CLI shorthand — `connector.cli`. It keys the IPC address and prefixes every fatal
/// error, so it reads like the rest of what the agent sees: `<cli>: <what went wrong>`.
const CLI: &str = env!("CLAPP_CLI");
/// The display name, for the window's wordmark and the snapshot.
const APP_NAME: &str = env!("CLAPP_NAME");

fn main() {
    // Windows draws this window with the Edge WebView2 Runtime, which is not part of the
    // app. Checked here, before Tauri looks for it, so a missing runtime is a sentence with
    // a download link instead of a modal dialog from a loader nobody has heard of. A no-op
    // everywhere else, and never on the CLI path — `gturag --help` needs no webview.
    clappkit::role::main_dispatch(APP_ID, CLI, cli::run, || {
        clappkit::webview::ensure(CLI);
        app::run()
    });
}
