//! clatch-ipc, the local control-channel substrate.
//!
//! One concept: a framed, ordered, bidirectional JSON-RPC 2.0 channel between two
//! local processes, over a unix domain socket (Linux/macOS) or a named pipe
//! (Windows). Clatch builds two vocabularies on top of it: the App control pipe
//! (`clatch-pipe`) and the client/daemon admin channel. This crate knows neither.
//!
//! Layers: [`frame`] (length-prefixed JSON) carries the `rpc` envelope, over a
//! `transport` socket, driven by a [`Peer`] pump.

pub mod frame;
mod peer;
mod rpc;
mod transport;

pub use frame::FrameLimits;
pub use peer::{Inbox, Peer};
pub use rpc::{Frame, Id, Kind, Notification, Request, Response, RpcError};
pub use transport::{connect, Listener};
