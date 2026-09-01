//! The app side of the pipe: the reference binding (reference/protocol.md
//! § Handshake). An app the launcher spawned reads its [`Identity`] from the env,
//! connects back, sends `register` (just the token), then serves Clatch's callbacks
//! (answer `ping`, exit on `shutdown`) while it may fire signals. This is the seed of
//! the per-language SDK; it links only `clatch-core`.

use crate::identity::Identity;
use crate::wire::{
    method, AgentsParams, ErrorCode, NotifyParams, RegisterParams, ToAgentParams,
    ToAgentRefusedParams,
};
use clatch_core::{ClatchError, ConnectedAgent, Result, SignalDecl, SignalType};
use clatch_ipc::{connect, frame, Frame, FrameLimits, Inbox, Kind, Peer, Request, Response};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// A connected, registered app-side control pipe.
pub struct Client {
    peer: Peer,
    inbox: Inbox,
    identity: Identity,
    /// The app's own declared signals (id -> type). The manifest is the authority
    /// Clatch validates against; this local table is only how the binding stamps the
    /// declared `type` onto each `app.toAgent` (reference/protocol.md § Signals). It
    /// mirrors the manifest's `connector.signals`, exactly as a native app's constants
    /// mirror its `clatch.json`.
    declared: Vec<SignalDecl>,
    /// The app's connected agents, kept fresh from `app.agents` pushes while
    /// [`serve`](Self::serve) runs (reference/protocol.md § Connected agents). An
    /// `Arc` so a real app can hold a read handle while it serves.
    agents: Arc<Mutex<Vec<ConnectedAgent>>>,
    /// Refusals received from `app.toAgentRefused` while [`serve`](Self::serve) runs
    /// (reference/protocol.md § Signals): each is a fan-out that was refused whole. A
    /// faithful app surfaces these to the user; the reference client just records them.
    refusals: Arc<Mutex<Vec<ToAgentRefusedParams>>>,
}

impl Client {
    /// Connect using the identity Clatch injected into this process. `None` if the
    /// process was not launched by Clatch (reference/protocol.md § Launch). `declared`
    /// is the app's own `connector.signals` (id + type), used to stamp the type on
    /// each emission.
    pub async fn from_env(declared: &[SignalDecl]) -> Result<Self> {
        let identity = Identity::from_env()
            .ok_or_else(|| ClatchError::Invalid("not launched by Clatch".into()))?;
        Self::connect(identity, declared).await
    }

    /// Connect to `identity.addr` and complete the `register` handshake. Register
    /// carries only the token (reference/protocol.md § Handshake); `declared` stays
    /// app-side, to stamp the type on each `app.toAgent`.
    pub async fn connect(identity: Identity, declared: &[SignalDecl]) -> Result<Self> {
        let addr = identity.addr.clone();
        let mut stream = connect(&addr)
            .await
            .map_err(|e| ClatchError::io(&addr, e))?;

        let params = RegisterParams {
            instance_token: identity.token.clone(),
        };
        let req = Request::new(1, method::REGISTER, to_value(params));
        frame::write(&mut stream, &req, FrameLimits::control().max_frame)
            .await
            .map_err(|e| ClatchError::io(&addr, e))?;

        let reply = frame::read::<_, Frame>(&mut stream, FrameLimits::control())
            .await
            .map_err(|e| ClatchError::io(&addr, e))?
            .ok_or_else(|| ClatchError::Invalid("host closed during register".into()))?;
        let resp = match reply.classify() {
            Some(Kind::Response(r)) => r,
            _ => {
                return Err(ClatchError::Invalid(
                    "register reply was not a response".into(),
                ))
            }
        };
        if let Some(e) = resp.error {
            return Err(ClatchError::Invalid(format!(
                "register rejected {}: {}",
                e.code, e.message
            )));
        }

        let (peer, inbox) = Peer::start(stream, addr, FrameLimits::control());
        Ok(Self {
            peer,
            inbox,
            identity,
            declared: declared.to_vec(),
            agents: Arc::new(Mutex::new(Vec::new())),
            refusals: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// The identity Clatch assigned this run.
    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    /// The app's connected agents, from the latest `app.agents` push (empty until the
    /// first one lands, right after register). The app picks a target by name from
    /// this list; an empty [`signal_to`](Self::signal_to) target broadcasts.
    pub fn agents(&self) -> Vec<ConnectedAgent> {
        self.agents.lock().unwrap().clone()
    }

    /// A shared read handle to the connected-agents list, so an app can consult it
    /// from another task while [`serve`](Self::serve) owns the client and keeps it
    /// fresh.
    pub fn agents_handle(&self) -> Arc<Mutex<Vec<ConnectedAgent>>> {
        Arc::clone(&self.agents)
    }

    /// The refusals this app has received (reference/protocol.md § Signals): each is
    /// an all-or-nothing fan-out that was refused whole, so nothing was delivered. A
    /// faithful app surfaces them to the user (e.g. via [`notify`](Self::notify)).
    pub fn refusals(&self) -> Vec<ToAgentRefusedParams> {
        self.refusals.lock().unwrap().clone()
    }

    /// A shared read handle to the refusals list, for an app that watches it from
    /// another task while [`serve`](Self::serve) keeps it fresh.
    pub fn refusals_handle(&self) -> Arc<Mutex<Vec<ToAgentRefusedParams>>> {
        Arc::clone(&self.refusals)
    }

    /// Fire a declared signal at the agent (fire-and-forget). The type is looked up
    /// from the app's declared table and stamped on the wire; Clatch re-validates it
    /// against the manifest (reference/protocol.md § Signals). Fails if `id` was not
    /// declared, since the binding cannot know its type.
    pub async fn signal(&self, id: &str, payload: Value) -> Result<()> {
        self.signal_to(id, payload, Vec::new()).await
    }

    /// Fire a declared signal at **specific target agents** by immutable **id**.
    /// Same as [`signal`](Self::signal) but delivery is restricted to `target`,
    /// still intersected with the cut matrix (the app can only reach an agent
    /// that granted it, reference/protocol.md § Signals). Empty `target` is the
    /// broadcast fan-out. The app chooses the targets; Clatch adds no bias (it
    /// injects `CLATCH_AGENT_ID` so an app can target the caller, a roster id
    /// for a named other, or everyone, all through this one call).
    pub async fn signal_to(&self, id: &str, payload: Value, target: Vec<String>) -> Result<()> {
        let signal_type = self.declared_type(id).ok_or_else(|| {
            ClatchError::Invalid(format!("refusing to emit undeclared signal '{id}'"))
        })?;
        self.peer
            .notify(
                method::TO_AGENT,
                to_value(ToAgentParams {
                    id: id.into(),
                    signal_type,
                    target,
                    payload,
                }),
            )
            .await
    }

    /// The declared type for `id`, or `None` if this app never declared it.
    fn declared_type(&self, id: &str) -> Option<SignalType> {
        self.declared
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.signal_type)
    }

    /// Post a short line to the user's chat (fire-and-forget).
    pub async fn notify(&self, text: &str) -> Result<()> {
        self.peer
            .notify(method::NOTIFY, to_value(NotifyParams { text: text.into() }))
            .await
    }

    /// Serve Clatch's callbacks until shutdown: answer `ping`, apply the connected
    /// agents roster, drain the rest, and return when asked to stop. A real app runs
    /// this alongside its own work; the reference client just keeps the pipe healthy.
    pub async fn serve(&mut self) -> Result<()> {
        loop {
            tokio::select! {
                req = self.inbox.requests.recv() => {
                    let Some(req) = req else { return Ok(()) };
                    match req.method.as_str() {
                        method::PING => self.peer.respond(Response::ok(req.id, json!({ "ok": true }))).await?,
                        method::SHUTDOWN => {
                            self.peer.respond(Response::ok(req.id, json!({}))).await?;
                            return Ok(());
                        }
                        _ => self.peer.respond(Response::err(req.id, ErrorCode::Malformed.rpc())).await?,
                    }
                }
                notif = self.inbox.notifications.recv() => {
                    let Some(n) = notif else { return Ok(()) }; // pipe closed
                    if n.method == method::AGENTS {
                        if let Ok(p) = serde_json::from_value::<AgentsParams>(n.params) {
                            // Full-snapshot-on-change: just replace the view. The
                            // ordered stream already delivers snapshots in order.
                            *self.agents.lock().unwrap() = p.agents;
                        }
                    } else if n.method == method::TO_AGENT_REFUSED {
                        if let Ok(p) = serde_json::from_value::<ToAgentRefusedParams>(n.params) {
                            self.refusals.lock().unwrap().push(p);
                        }
                    }
                    // Any other callback: a minimal client drains it.
                }
            }
        }
    }
}

fn to_value<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("control pipe params serialize")
}
