//! The transparent timeline (reference/daemon.md, section Consistency; the
//! transparency rule is reference/signals.md). One totally ordered record of
//! everything that touched an agent: inputs (with their mode) and the canonical
//! events its turns produced. The agent context is the source of truth; the
//! timeline is the live view over it.

use crate::agent::{Input, Source};
use crate::event::Event;
use crate::ids::AgentId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How an input is delivered (reference/signals.md): `Run` triggers a turn,
/// `Context` is a quiet insert (no turn). The router decides; the host executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Run,
    Context,
}

/// One waiting input in an agent's queue view (reference/daemon.md § Queue
/// introspection): everything accepted but not started, plus the router's
/// buffered context signals. The in-flight turn is NOT here (cancel that
/// instead). `ticket` is the id the run's rows will carry once it starts, so a
/// removed input never becomes a turn and the timeline stays truthful; it is
/// `None` for a context signal still in the router's buffer (visible, not yet
/// removable). `source` attributes the row (user / app / agent), the same
/// transparency the timeline gives delivered inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedInput {
    #[serde(default)]
    pub ticket: Option<u64>,
    pub text: String,
    pub mode: Mode,
    #[serde(default)]
    pub source: Source,
}

/// A declared signal's TYPE (reference/signals.md), fixed in the manifest at
/// declaration, never chosen per emission: `Run` triggers a turn, `Context`
/// queues (ordered, lossless) into the next turn, `Buffered` updates the
/// agent's visible chat buffer and rides only when the user sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalType {
    Run,
    Context,
    Buffered,
}

/// One declared signal in the manifest / registry record: its stable **id** and
/// its declared [`SignalType`] (reference/data-structures.md, reference/protocol.md
/// § Signals). `id` is the identifier an `app.toAgent` frame carries and the router
/// matches against; it is the whole vocabulary, never a per-emission counter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalDecl {
    pub id: String,
    #[serde(rename = "type")]
    pub signal_type: SignalType,
}

/// A signal **blocklist**, one layer of the cut matrix (reference/signals.md
/// § Signal filters): a signal is delivered iff no layer's cuts catch it. Lives
/// in app settings (app-level), the agent overlay (agent-global and per-bind),
/// and on the wire (the GUI edits it). Independent per-TYPE flags; the GUI's
/// four-rung ladder (Off < Buffered < Context < Run) is a view over them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SignalCuts {
    /// Cut everything.
    pub all: bool,
    /// Cut every `run`-type signal.
    pub run: bool,
    /// Cut every `context`-type signal.
    pub context: bool,
    /// Cut every `buffered`-type signal.
    pub buffered: bool,
    /// Cut these named signals.
    pub signals: Vec<String>,
}

impl SignalCuts {
    /// Does this layer block a signal `name` of declared type `t`?
    pub fn blocks(&self, name: &str, t: SignalType) -> bool {
        self.all
            || match t {
                SignalType::Run => self.run,
                SignalType::Context => self.context,
                SignalType::Buffered => self.buffered,
            }
            || self.signals.iter().any(|s| s == name)
    }

    /// Nothing cut (the default).
    pub fn is_open(&self) -> bool {
        !self.all && !self.run && !self.context && !self.buffered && self.signals.is_empty()
    }
}

/// Where an agent is right now, derived from its real lifecycle, never guessed
/// (reference/daemon.md, section Run-state). `Disconnected` joins when the
/// engine exposes backend health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Idle,
    Running,
}

/// One timeline row: `{ seq, ts, agent, turn, kind }`. `seq` is one counter across
/// all agents (assigned at emission), so the merged timeline has a total order.
/// `turn` groups a run turn's rows (its input, events, and terminal `Done`) under
/// one id, so a caller can follow exactly one turn among interleaved ones; it is
/// `None` for context inserts (no turn).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub agent: AgentId,
    #[serde(default)]
    pub turn: Option<u64>,
    pub kind: TimelineKind,
}

/// What a row records: an input arriving (attributed by source, marked by mode)
/// or a canonical event out of a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimelineKind {
    Input { input: Input, mode: Mode },
    Event(Event),
}
