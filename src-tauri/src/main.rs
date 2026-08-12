//! GTÜ Formlar — one binary, two roles over one state.
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

/// Must equal `clatch.json`'s `id`; `bootstrap` hard-errors on a mismatch.
const APP_ID: &str = "com.gtu.rag";
/// The CLI shorthand — `connector.cli`. It keys the IPC address and prefixes every fatal
/// error, so it reads like the rest of what the agent sees: `gturag: <what went wrong>`.
const CLI: &str = "gturag";

fn main() {
    clappkit::role::main_dispatch(APP_ID, CLI, cli::run, app::run);
}
