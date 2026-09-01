//! The Agent port, the clean abstraction Clatch drives every turn through
//! (reference/harnesses.md). Clatch owns this interface and the canonical
//! [`Event`](crate::event::Event); the implementation (agent-engine, the debug
//! agent, clatch-harness later) is external and swappable.
//!
//! The port is deliberately tiny: a turn (`run`), passive context (`insert`),
//! and permissions (`grant`/`revoke`, the tool-grant mechanic of
//! reference/tools.md). Richer ops (fork, customize, destroy) live on the
//! agent-engine API and only join the port when a consumer needs them.

use crate::error::Result;
use crate::event::EventStream;
use crate::ids::AppId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

/// Who triggered a run / context-insert. `Agent` is ATOA (reference/signals.md
/// § ATOA): another agent sent it via the launcher CLI, identity-injected.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum Source {
    #[default]
    Human,
    App(AppId),
    Agent(crate::ids::AgentId),
    Clatch,
}

/// One model a backend offers (GUI: the Agent Settings model picker). `id` is
/// what [`Agent::set_model`] takes; `current` marks the active one when the
/// backend reports it (the GUI can also derive it from the agent's set model).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub current: bool,
}

/// An agent's profile photo as served to clients (reference/data-structures.md
/// § Agent avatar): the metadata plus the same-machine path a client reads the
/// bytes from (the GUI loads it directly, as it does app icons). The engine owns
/// the file + the standard; this is its wire form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Avatar {
    pub mime: String,
    /// Absolute path to the image file.
    pub path: String,
    pub width: u32,
    pub height: u32,
}

/// One agent bound to an app, as pushed to that app (reference/protocol.md
/// § Connected agents): the immutable `id` (the ONLY wire key an app may
/// target or store under), the display `name` (unique but re-pointable; same
/// id + new name = the same agent re-labeled), its backend/brand, current
/// model, and avatar. Deliberately minimal: an app never learns an agent's
/// permissions, cuts, or the OTHER apps it is bound to (the trust boundary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectedAgent {
    pub id: crate::ids::AgentId,
    pub name: String,
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<Avatar>,
}

/// One advertised sign-in for a backend (ACP `AuthMethod`): id + human name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthChoice {
    pub id: String,
    pub name: String,
}

/// How a backend signs in, so a client renders login without a hardcoded
/// table. `terminal` = a wrapped vendor CLI's TUI (the caller opens a console);
/// else Clatch drives ACP login and `methods` are the choices to offer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginShape {
    pub terminal: bool,
    pub methods: Vec<AuthChoice>,
}

/// Why an installed backend is not usable, classified from the adapter's own
/// error text (reference/install.md § states). Fail-soft by construction: an
/// unrecognized error lands on `Bug` carrying the verbatim message, never a
/// wrong bucket. The engine owns the classification; this is its wire form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnavailableReason {
    UsageLimit,
    Offline,
    NoAccess,
    UpdateRequired,
    Bug,
}

/// The runtime state of an INSTALLED backend, from one models-roundtrip probe
/// (`backend.state`). Distinct from the persisted install state (`backend.list`):
/// this one costs an adapter spawn, so it is probed on demand, never polled.
/// Serializes on `state` so a client reads `{state, reason?, detail?}` flat.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum BackendState {
    /// The adapter answered: logged in and usable.
    Ready,
    /// The adapter advertises sign-in and the credential is missing.
    LoginRequired,
    /// Logged in but not usable; `detail` is the adapter's verbatim message.
    Unavailable {
        reason: UnavailableReason,
        detail: String,
    },
}

/// One selectable value of a session config option.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigValue {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A session config option the agent's backend exposes (the `select` kind),
/// e.g. the mode (reference/agent-engine.md § Session config). `category` is
/// the backend's UX hint ("mode", "thought_level", ...); `selected` its
/// current value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigOption {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub selected: Option<String>,
    pub values: Vec<ConfigValue>,
}

/// A content-addressed reference to an image the user attached to a human turn
/// (the composer's paste/drop): `id` is the content hash, `mime` its media
/// type. The bytes live in the agent's image store; the timeline carries only
/// this ref (never the bytes), so the in-memory ring stays small. Only a human
/// run input carries these; an app can never send an image (reference/signals.md).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputImage {
    pub id: String,
    pub mime: String,
}

/// Input to [`Agent::run`] (a turn) or [`Agent::insert`] (passive context).
/// `depth` is the ATOA chain bound (reference/signals.md § ATOA): 0 for human
/// and app inputs, sender's in-flight depth + 1 for an agent-sent one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Input {
    pub source: Source,
    pub text: String,
    #[serde(default)]
    pub depth: u32,
    /// User-attached images (empty for app/agent/context inputs); ride the
    /// prompt as ACP image content, capability permitting.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<InputImage>,
}

impl Input {
    pub fn new(source: Source, text: impl Into<String>) -> Self {
        Self {
            source,
            text: text.into(),
            depth: 0,
            images: Vec::new(),
        }
    }

    /// The same input carrying user-attached images (the composer path).
    pub fn with_images(mut self, images: Vec<InputImage>) -> Self {
        self.images = images;
        self
    }

    /// The same input at an explicit ATOA chain depth.
    pub fn at_depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }
}

#[async_trait]
pub trait Agent: Send + Sync {
    /// Run one turn; stream canonical events. Cancellation is via `cancel`. (mode: run)
    async fn run(&self, input: Input, cancel: CancellationToken) -> Result<EventStream>;

    /// Append passive context, no turn. (mode: context)
    async fn insert(&self, input: Input) -> Result<()>;

    /// Allow a permission pattern, e.g. `Bash(<cli>:*)` (reference/tools.md).
    async fn grant(&self, pattern: &str) -> Result<()>;

    /// Withdraw a permission pattern. Idempotent: unknown patterns are a no-op.
    async fn revoke(&self, pattern: &str) -> Result<()>;

    /// Answer a parked permission ask (reference/permissions.md), by the `id`
    /// of its [`Event::Permission`](crate::Event::Permission). Must never wait
    /// on the in-flight turn (the turn is waiting on *this*). Errs when the id
    /// is unknown or already resolved (a benign race; the first answer wins).
    async fn answer(&self, id: &str, allow: bool) -> Result<()>;

    /// The current permission allowlist, for display (reference/permissions.md).
    async fn permissions(&self) -> Result<Vec<String>>;

    /// Store user-attached images (`(mime, base64)` pairs) in the agent's blob
    /// store and return their content-addressed refs (the shape the timeline
    /// and transcript carry). Default: none (an agent with no image store, e.g.
    /// the debug console).
    async fn persist_images(&self, _images: &[(String, String)]) -> Result<Vec<InputImage>> {
        Ok(Vec::new())
    }

    /// Read a stored image's base64 payload by id (the GUI's fetch). Default:
    /// not found (no store).
    async fn image_base64(&self, id: &str) -> Result<String> {
        Err(crate::ClatchError::NotFound(format!("image {id}")))
    }

    /// Set (or clear) the model override; applies from the next turn. The
    /// backend is deliberately NOT settable: an agent's backend is fixed at
    /// creation (GUI: Agent Settings).
    async fn set_model(&self, model: Option<String>) -> Result<()>;

    /// Set (or clear) the user's instructions (system prompt); applies from the
    /// next turn (GUI: Agent Settings, "Instructions").
    async fn set_instructions(&self, text: Option<String>) -> Result<()>;

    /// Set the agent's profile photo from `bytes` (reference/data-structures.md
    /// § Agent avatar): the backend validates against the standard and stores it,
    /// returning the metadata + same-machine path. Default: unsupported (a
    /// substrate-free backend, the debug console, has nowhere to keep it).
    async fn set_avatar(&self, _bytes: Vec<u8>) -> Result<Avatar> {
        Err(crate::ClatchError::Invalid(
            "this agent cannot hold an avatar".into(),
        ))
    }

    /// Clear the agent's profile photo (a no-op if there is none). Default:
    /// unsupported, matching [`Agent::set_avatar`].
    async fn clear_avatar(&self) -> Result<()> {
        Err(crate::ClatchError::Invalid(
            "this agent cannot hold an avatar".into(),
        ))
    }

    /// The models this agent's backend offers (GUI: the model picker). Default:
    /// none, for a model-free agent (the debug console). A real backend override
    /// lists them; the call may be slow (a backend roundtrip), so a caller runs
    /// it off the daemon actor (reference/daemon.md).
    async fn models(&self) -> Result<Vec<ModelInfo>> {
        Ok(Vec::new())
    }

    /// The backend's session config options (the `select` kind, e.g. the
    /// mode). Default: none - nothing configurable is a normal state, never an
    /// error. Same off-actor rule as `models` (a backend roundtrip).
    async fn config(&self) -> Result<Vec<ConfigOption>> {
        Ok(Vec::new())
    }

    /// Choose (or clear, with `None`) a session config value; persists and
    /// applies from the next turn. Default: unsupported (the debug console has
    /// nothing to configure).
    async fn set_config(&self, _id: &str, _value: Option<String>) -> Result<()> {
        Err(crate::ClatchError::Invalid(
            "this agent has no session config".into(),
        ))
    }

    /// Older conversation rows for scroll-up paging (reference/daemon.md,
    /// `agent.history`): up to `limit` message-grain rows STRICTLY older than
    /// `before`, oldest-first. Default: no persisted conversation (the debug
    /// echo), so the page is empty and the client shows the beginning.
    async fn history_before(
        &self,
        _before: chrono::DateTime<chrono::Utc>,
        _limit: usize,
    ) -> Result<Vec<(chrono::DateTime<chrono::Utc>, crate::TimelineKind)>> {
        Ok(Vec::new())
    }
}
