//! The App control pipe vocabulary (reference/protocol.md § The API): the method
//! names, the parameter payloads, and the error codes carried over the generic
//! JSON-RPC envelope (`clatch_ipc`). The app sends `register` plus fire-and-forget
//! notifications; Clatch sends callbacks. Adding a method is a deliberate act.

use clatch_core::SignalType;
use clatch_ipc::RpcError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Method names. The whole vocabulary; nothing else crosses the pipe.
pub mod method {
    /// app -> clatch, the handshake (the first and only request the app issues).
    pub const REGISTER: &str = "app.register";
    /// app -> clatch, fire a declared signal at the agent(s) (notification).
    pub const TO_AGENT: &str = "app.toAgent";
    /// app -> clatch, a short line for the user's chat (notification, optional).
    pub const NOTIFY: &str = "app.notify";
    /// clatch -> app, graceful stop; the app replies then exits cleanly.
    pub const SHUTDOWN: &str = "app.shutdown";
    /// clatch -> app, an on-demand health probe (never a timer).
    pub const PING: &str = "app.ping";
    /// clatch -> app, the app's connected agents (notification), pushed once
    /// after register and again on every change (reference/protocol.md
    /// § Connected agents). Params [`AgentsParams`](super::AgentsParams).
    pub const AGENTS: &str = "app.agents";
    /// clatch -> app, an all-or-nothing fan-out was refused whole (notification):
    /// a receiver could not accept a `run`/`context` signal, so nothing was
    /// delivered (reference/protocol.md § Signals). Params
    /// [`ToAgentRefusedParams`](super::ToAgentRefusedParams).
    pub const TO_AGENT_REFUSED: &str = "app.toAgentRefused";
}

/// The control pipe's error kinds (reference/protocol.md § Error codes). Errors
/// exist only for requests (`register`); signals are fire-and-forget and a
/// violation is dropped launcher-side, never answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// register's token does not match what Clatch injected.
    IdentityMismatch,
    /// unparseable or schema-invalid message.
    Malformed,
}

impl ErrorCode {
    pub fn code(self) -> i64 {
        match self {
            ErrorCode::IdentityMismatch => 1001,
            ErrorCode::Malformed => 1002,
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            ErrorCode::IdentityMismatch => "identity mismatch",
            ErrorCode::Malformed => "malformed message",
        }
    }
    pub fn rpc(self) -> RpcError {
        RpcError {
            code: self.code(),
            message: self.message().into(),
        }
    }
}

/// `app.register` params (reference/protocol.md § Handshake). Only the token: the
/// per-instance socket already names the instance, the manifest holds the app's
/// identity and signals, and the protocol major is validated at install. The token
/// is the one thing Clatch cannot already know (it proves "I am the process you
/// spawned"). Standalone dev sends an empty token.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterParams {
    pub instance_token: String,
}

/// `app.toAgent` params (reference/protocol.md § Signals): a declared signal `id`,
/// the `type` stamped from its manifest declaration (re-validated launcher-side and
/// dropped on mismatch), the target agents, and an arbitrary-JSON payload. There is
/// no per-emission id and no sequence number: the stream is ordered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToAgentParams {
    /// The signal's stable declared identifier (matched against the manifest).
    pub id: String,
    /// The declared type the app claims. Clatch re-validates it against the
    /// manifest and drops the signal if it disagrees, so the wire states intent
    /// checkably without becoming the authority.
    #[serde(rename = "type")]
    pub signal_type: SignalType,
    /// Explicit **target agents** by immutable **id** (reference/protocol.md
    /// § Signals; the display name is never a wire key). Empty = the default
    /// fan-out to every bound agent. Non-empty = deliver only to these, still
    /// intersected with the cut matrix (an app can only reach an agent that
    /// granted it, the permission boundary). The app decides who to target;
    /// Clatch imposes no bias (it injects `CLATCH_AGENT_ID` for "the caller",
    /// and the `app.agents` roster maps id to display name for a named other).
    #[serde(default)]
    pub target: Vec<String>,
    /// Arbitrary JSON, the app-defined content the signal carries. Clatch renders
    /// it for the agent, never interprets it.
    #[serde(default)]
    pub payload: Value,
}

/// `app.notify` params: a short line for the user's chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyParams {
    pub text: String,
}

/// `app.agents` params (clatch -> app): the app's connected agents, a full snapshot
/// each time (the app replaces its view). No seq: the ordered stream already
/// delivers snapshots in order (reference/protocol.md § Connected agents).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentsParams {
    pub agents: Vec<clatch_core::ConnectedAgent>,
}

/// `app.toAgentRefused` params (clatch -> app): an all-or-nothing `run`/`context`
/// fan-out was refused whole because a receiver could not accept it, so nothing was
/// delivered (reference/protocol.md § Signals). `id` is the refused signal's
/// declared id; `agent` is the first receiver (name order) that could not accept;
/// `reason` is `inbox_full` (a `run` target) or `queue_full` (a `context` target).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToAgentRefusedParams {
    pub id: String,
    pub agent: String,
    pub reason: String,
}
