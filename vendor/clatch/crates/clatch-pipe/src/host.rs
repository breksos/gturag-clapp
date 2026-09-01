//! The launcher side of the pipe (reference/protocol.md). [`Pipe::accept`] takes a
//! bound [`Listener`], waits for the spawned app, runs the `register` handshake
//! (proving the app's token against what Clatch injected), and on success hands back
//! a `Pipe` to ping, signal-receive, push the agents roster, notify a refusal, and
//! shut down.

use crate::identity::Identity;
use crate::wire::{
    method, AgentsParams, ErrorCode, NotifyParams, RegisterParams, ToAgentParams,
    ToAgentRefusedParams,
};
use clatch_core::{AppId, ClatchError, ConnectedAgent, Result, SignalDecl};
use clatch_ipc::{frame, Frame, FrameLimits, Inbox, Kind, Listener, Peer, Response};
use serde_json::json;
use std::time::Duration;

/// Something the app sent us over the pipe (reference/protocol.md § The API).
/// Signals are advisory; the agent host decides whether to act.
pub enum Inbound {
    ToAgent(ToAgentParams),
    Notify(NotifyParams),
}

/// One launcher-side control connection to a running app.
pub struct Pipe {
    peer: Peer,
    inbox: Inbox,
    app_id: AppId,
    /// The manifest's declared signals (id + type), the authority `recv` checks an
    /// `app.toAgent` against: an undeclared id, or a wire type that disagrees with
    /// the declaration, is dropped (reference/protocol.md § Signals).
    declared: Vec<SignalDecl>,
}

impl Pipe {
    /// Accept the spawned app and complete the `register` handshake. Fails if the app
    /// never connects, sends something other than `register` first, or presents a
    /// token that does not match the one Clatch injected.
    pub async fn accept(
        listener: &Listener,
        identity: &Identity,
        declared: &[SignalDecl],
    ) -> Result<Self> {
        let addr = identity.addr.clone();
        let mut stream = listener
            .accept()
            .await
            .map_err(|e| ClatchError::io(&addr, e))?;

        let frame = frame::read::<_, Frame>(&mut stream, FrameLimits::control())
            .await
            .map_err(|e| ClatchError::io(&addr, e))?
            .ok_or_else(|| ClatchError::Invalid("app closed before register".into()))?;

        let req = match frame.classify() {
            Some(Kind::Request(r)) if r.method == method::REGISTER => r,
            _ => {
                return Err(ClatchError::Invalid(
                    "first message was not app.register".into(),
                ))
            }
        };

        let params: RegisterParams = serde_json::from_value(req.params)
            .map_err(|e| ClatchError::Invalid(format!("register params: {e}")))?;

        // The per-instance socket already names the instance and the manifest holds
        // the app's identity; the token is the one thing that must be proven here.
        if params.instance_token != identity.token {
            let _ = frame::write(
                &mut stream,
                &Response::err(req.id, ErrorCode::IdentityMismatch.rpc()),
                FrameLimits::control().max_frame,
            )
            .await;
            return Err(ClatchError::Invalid("register: identity mismatch".into()));
        }

        // A small, forward-safe object the app may log or ignore (reference/protocol.md
        // § Handshake); today just the launcher's version.
        let host_context = json!({ "hostContext": { "clatch": env!("CARGO_PKG_VERSION") } });
        frame::write(
            &mut stream,
            &Response::ok(req.id, host_context),
            FrameLimits::control().max_frame,
        )
        .await
        .map_err(|e| ClatchError::io(&addr, e))?;

        let (peer, inbox) = Peer::start(stream, addr, FrameLimits::control());
        Ok(Self {
            peer,
            inbox,
            app_id: identity.app_id.clone(),
            declared: declared.to_vec(),
        })
    }

    /// The app this pipe belongs to.
    pub fn app_id(&self) -> &AppId {
        &self.app_id
    }

    /// Push the app's connected-agents snapshot (reference/protocol.md § Connected
    /// agents): a fire-and-forget `app.agents` notification. NON-BLOCKING and
    /// best-effort (R1, 2026-07-23): if the app is not draining its socket the push
    /// is dropped rather than parking the supervisor (which would let a live-but-not-
    /// reading app wedge the daemon), and the next change re-pushes the truth.
    pub fn push_agents(&self, agents: Vec<ConnectedAgent>) {
        let params = serde_json::to_value(AgentsParams { agents }).unwrap_or(json!({}));
        self.peer.notify_lossy(method::AGENTS, params);
    }

    /// Tell the emitting app an all-or-nothing fan-out was refused whole
    /// (reference/protocol.md § Signals): a fire-and-forget `app.toAgentRefused`.
    /// Non-blocking and best-effort (R1), like [`push_agents`](Self::push_agents).
    pub fn push_refused(&self, id: String, agent: String, reason: String) {
        let params =
            serde_json::to_value(ToAgentRefusedParams { id, agent, reason }).unwrap_or(json!({}));
        self.peer.notify_lossy(method::TO_AGENT_REFUSED, params);
    }

    /// Health probe; `Ok` if the app answers within `timeout`.
    pub async fn ping(&self, timeout: Duration) -> Result<()> {
        let resp = self.peer.request(method::PING, json!({}), timeout).await?;
        if resp.is_ok() {
            Ok(())
        } else {
            Err(rpc_err(&resp))
        }
    }

    /// Ask the app to exit cleanly. The app may reply, or simply drop the socket;
    /// both mean it is going away, so both count as success. Only a timeout fails.
    pub async fn shutdown(&self, timeout: Duration) -> Result<()> {
        match self
            .peer
            .request(method::SHUTDOWN, json!({}), timeout)
            .await
        {
            Ok(resp) if resp.is_ok() => Ok(()),
            Ok(resp) => Err(rpc_err(&resp)),
            Err(ClatchError::Io { .. }) => Ok(()), // socket closed: the app exited
            Err(e) => Err(e),
        }
    }

    /// The next thing the app sent, or `None` when the pipe closes. An `app.toAgent`
    /// whose id the manifest did not declare, or whose wire type disagrees with the
    /// declaration, is dropped here (advisory, never an error reply on a
    /// fire-and-forget); the rest of the routing lives in the core.
    ///
    /// The app->clatch vocabulary is notifications only (toAgent, notify); an app
    /// that sends a JSON-RPC *request* gets an error reply, so the pump's bounded
    /// requests channel is DRAINED, not left to fill. Without this a misbehaving app
    /// that sends 64+ requests wedges its own pipe (the pump blocks writing into the
    /// never-read channel), silencing its signals and blocking Clatch's own
    /// ping/shutdown (reference/protocol.md).
    pub async fn recv(&mut self) -> Option<Inbound> {
        loop {
            tokio::select! {
                // Bias notifications: they are the real traffic; requests are the
                // unexpected case we only need to keep drained.
                biased;
                req = self.inbox.requests.recv() => {
                    let req = req?;
                    let _ = self
                        .peer
                        .respond(Response::err(req.id, ErrorCode::Malformed.rpc()))
                        .await;
                }
                notif = self.inbox.notifications.recv() => {
                    let n = notif?;
                    match n.method.as_str() {
                        method::TO_AGENT => {
                            if let Ok(p) = serde_json::from_value::<ToAgentParams>(n.params) {
                                // The declaration is the authority: the id must be
                                // declared AND its wire type must match. A mismatch
                                // or an undeclared id is dropped.
                                if self
                                    .declared
                                    .iter()
                                    .any(|d| d.id == p.id && d.signal_type == p.signal_type)
                                {
                                    return Some(Inbound::ToAgent(p));
                                }
                            }
                        }
                        method::NOTIFY => {
                            if let Ok(p) = serde_json::from_value::<NotifyParams>(n.params) {
                                return Some(Inbound::Notify(p));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn rpc_err(resp: &Response) -> ClatchError {
    match &resp.error {
        Some(e) => ClatchError::Invalid(format!("pipe error {}: {}", e.code, e.message)),
        None => ClatchError::Invalid("pipe error".into()),
    }
}
