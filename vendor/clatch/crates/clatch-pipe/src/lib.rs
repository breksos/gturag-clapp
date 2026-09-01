//! clatch-pipe, the **official Rust reference binding** for the Clatch <-> App
//! control pipe (reference/protocol.md, § The reference binding). Clatch's
//! `steam_api` equivalent: the **wire is the contract**, and this crate is the
//! recommended, optional convenience that speaks it for you. It links only
//! `clatch-core` + `clatch-ipc`, so an app depends on it without the launcher; other
//! languages implement the wire directly (it is small and fully specified).
//!
//! The channel is narrow, ordered, and versioned; it carries only handshake,
//! identity, signals, health, and shutdown. It is **not** the app's own GUI <-> CLI
//! pipe (that is the developer's).
//!
//! # App author's surface
//!
//! - [`clatch_init`]: the launch bootstrap (reference/launch.md): run only under
//!   Clatch, relaunching through it otherwise.
//! - [`Client`]: connect with the injected identity + `register`, then `signal` /
//!   `notify` / `serve` (answer `ping`, exit on `shutdown`).
//! - [`Identity`]: the per-run identity Clatch injected (usually read for you by
//!   [`Client::from_env`]).
//!
//! ```no_run
//! use clatch_core::{SignalDecl, SignalType};
//! use clatch_pipe::{clatch_init, Client};
//!
//! # async fn app() -> clatch_core::Result<()> {
//! // Runs only under Clatch: hand off and exit if started any other way.
//! if clatch_init("com.you.todo")? {
//!     return Ok(());
//! }
//! // Connect back. The declared signals (id + type) mirror clatch.json, so the
//! // binding stamps each signal's type on the wire.
//! let declared = vec![SignalDecl { id: "item.added".into(), signal_type: SignalType::Run }];
//! let mut client = Client::from_env(&declared).await?;
//! client
//!     .signal("item.added", serde_json::json!({ "id": 1 }))
//!     .await?;
//! client.serve().await?; // answer ping, return on shutdown
//! # Ok(())
//! # }
//! ```
//!
//! The worked reference app is `refapp`
//! (`clatch-lifecycle/examples/refapp.rs`).
//!
//! # The launcher's side
//!
//! [`Pipe`] and [`Inbound`] are the *launcher's* half (accept a spawned app and
//! drive it); app authors do not use them, only Clatch itself does. The whole
//! vocabulary lives in [`wire`] (methods, params, error codes).

mod bootstrap;
mod client;
mod host;
mod identity;
pub mod wire;

pub use bootstrap::{clatch_init, ENV_BIN, ENV_STANDALONE};
pub use client::Client;
pub use host::{Inbound, Pipe};
pub use identity::Identity;
