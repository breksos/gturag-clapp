//! clatch-core, the canonical foundation.
//!
//! Pure types + traits, no I/O: stable [`ids`], the canonical [`Event`], the
//! [`Agent`] port, the transparent [`timeline`], and a model-free [`DebugAgent`].
//! Everything Clatch builds on. See `reference/harnesses.md` (the Agent port) and
//! `reference/daemon.md` (the timeline).

pub mod agent;
pub mod debug;
pub mod element;
pub mod error;
pub mod event;
pub mod fs;
pub mod ids;
pub mod persist;
pub mod process;
pub mod progress;
pub mod shim;
pub mod timeline;

pub use agent::{
    Agent, AuthChoice, Avatar, BackendState, ConfigOption, ConfigValue, ConnectedAgent, Input,
    InputImage, LoginShape, ModelInfo, Source, UnavailableReason,
};
pub use debug::DebugAgent;
pub use element::ElementType;
pub use error::{ClatchError, Result};
pub use event::{Event, EventStream, Outcome};
pub use ids::{valid_segment, AgentId, AgentName, AppId};
pub use progress::Progress;
pub use timeline::{
    Mode, QueuedInput, RunState, SignalCuts, SignalDecl, SignalType, TimelineEntry, TimelineKind,
};

/// The `CLATCH_BIN` override: the path of the `clatch` binary, for anything
/// that must spawn it (the GUI's daemon launch, an app's managed relaunch).
/// One constant so no binary carries the name as a magic string.
pub const ENV_BIN: &str = "CLATCH_BIN";

/// The app's durable-data home, injected at spawn (`~/.clatch/appdata/<id>`,
/// reference/protocol.md § Transport): the ONE place a clapp writes persistent
/// state, so an uninstall with `purge` can erase the whole footprint.
pub const ENV_DATA_DIR: &str = "CLATCH_DATA_DIR";

/// The identity a tool inherits from the agent that runs it: the agent's
/// immutable [`AgentId`], injected into every adapter session and therefore
/// every tool process (reference/signals.md § ATOA, tools.md). Being the id,
/// it stays valid across a name re-point for as long as the agent exists.
pub const ENV_AGENT_ID: &str = "CLATCH_AGENT_ID";

/// The Windows named-pipe namespace every Clatch endpoint lives under (the
/// daemon's admin pipe and each app instance's control pipe append their own
/// distinct suffixes). One constant so the namespace cannot be renamed in one
/// place and not the other.
pub const PIPE_PREFIX: &str = r"\\.\pipe\clatch-";

/// The control-pipe major this launcher supports (reference/protocol.md
/// § Versioning). A single integer; within a major only additive change. It is
/// validated at **install** against a manifest's `protocol`, so a running instance
/// is compatible by construction and register negotiates nothing.
pub const SUPPORTED_PROTOCOL: u32 = 2;
