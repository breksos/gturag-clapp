//! The canonical `Event`, the only vocabulary the Agent host consumes
//! (reference/harnesses.md). Every executor maps its own output into this.
//!
//! Only variants a producer actually emits live here. `AppNotification` joins when
//! the router emits it.

use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Text(String),
    /// An image the agent emitted (its message stream, or a tool's reported
    /// content): a content-addressed ref into the agent's blob store, never
    /// the bytes (the ring, the wire, and replays stay small). Clients fetch
    /// the base64 payload by id through `agent.image`, exactly like an input
    /// attachment's ref (reference/agent-engine.md § Images).
    Image {
        id: String,
        mime: String,
    },
    Thinking(String),
    ToolCall {
        /// Correlates the coming `ToolResult` updates to this call.
        id: String,
        name: String,
        args: serde_json::Value,
    },
    /// A tool call's status/result, keyed by the same `id` as its `ToolCall`.
    /// Carries the raw output when the executor reports it (the GUI shows it).
    ToolResult {
        id: String,
        status: String,
        output: Option<serde_json::Value>,
    },
    /// The turn's resource report (reference/harnesses.md): context occupancy
    /// against the window, and the session's cumulative cost when the vendor
    /// reports it. Feeds the GUI context meter off the timeline.
    Usage {
        context_tokens: u64,
        context_window: Option<u64>,
        cost_usd: Option<f64>,
    },
    /// The agent asked permission for a tool outside its allowlist
    /// (reference/permissions.md). The turn parks until
    /// [`Agent::answer`](crate::Agent::answer) resolves it; every unresolved
    /// path denies (unattended, timeout, turn end). `kind` is the normalized
    /// tool class (`read`/`edit`/`execute`/...); `command` is the command line
    /// for execute.
    Permission {
        id: String,
        kind: String,
        command: String,
    },
    Done(Outcome),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Outcome {
    Ok,
    Failed(String),
    ReasoningOnly,
    /// The turn was aborted (user cancel, any trigger source); nothing persisted.
    Cancelled,
}

/// A live stream of canonical events from one turn.
pub type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;
