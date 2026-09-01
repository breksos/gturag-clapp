//! A model-free implementation of the [`Agent`] port. `run` echoes the input as
//! canonical events; `insert` records context. It proves the port is clean and
//! lets the whole Clatch stack run offline (reference/harnesses.md, debug console).
//!
//! The ask model (reference/permissions.md) is exercisable offline too: an input
//! `ask:<kind>:<command>` makes the turn emit [`Event::Permission`] and park until
//! [`Agent::answer`] resolves it (or a bounded wait denies), then report the
//! outcome as text, exactly the shape a real executor produces.

use crate::agent::{Agent, Input};
use crate::error::{ClatchError, Result};
use crate::event::{Event, EventStream, Outcome};
use async_trait::async_trait;
use futures::channel::oneshot;
use futures::{stream, StreamExt};
use std::collections::HashMap;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct DebugAgent {
    context: Mutex<Vec<Input>>,
    grants: Mutex<Vec<String>>,
    asks: Mutex<Asks>,
}

#[derive(Default)]
struct Asks {
    next: u64,
    waiting: HashMap<String, oneshot::Sender<bool>>,
}

impl DebugAgent {
    pub fn new() -> Self {
        Self::default()
    }

    /// The context inserted so far (for tests / inspection).
    pub fn context(&self) -> Vec<Input> {
        self.context.lock().expect("debug context lock").clone()
    }

    /// The permission patterns granted so far (for tests / inspection).
    pub fn grants(&self) -> Vec<String> {
        self.grants.lock().expect("debug grants lock").clone()
    }

    fn park(&self) -> (String, oneshot::Receiver<bool>) {
        let mut asks = self.asks.lock().expect("debug asks lock");
        asks.next += 1;
        let id = format!("ask-{}", asks.next);
        let (tx, rx) = oneshot::channel();
        asks.waiting.insert(id.clone(), tx);
        (id, rx)
    }
}

#[async_trait]
impl Agent for DebugAgent {
    async fn run(&self, input: Input, cancel: CancellationToken) -> Result<EventStream> {
        // `ask:<kind>:<command>`: raise a permission ask mid-turn (the offline
        // twin of an ACP request_permission outside the allowlist). Tolerates a
        // space after the prefix, so a routed signal `ask: <data>` works too.
        if let Some(rest) = input.text.strip_prefix("ask:") {
            let rest = rest.trim_start().to_string();
            let (kind, command) = rest.split_once(':').unwrap_or((rest.as_str(), ""));
            let (id, rx) = self.park();
            let ask = Event::Permission {
                id,
                kind: kind.to_string(),
                command: command.to_string(),
            };
            // Parks until answered; a dropped sender or a cancel reads as deny.
            // No timeout here: the real executor owns ASK_TIMEOUT, and the
            // daemon's unattended policy answers at once (reference/permissions.md).
            let outcome = stream::once(async move {
                let answered = std::pin::pin!(rx);
                let cancelled = std::pin::pin!(cancel.cancelled());
                let allowed = match futures::future::select(answered, cancelled).await {
                    futures::future::Either::Left((answer, _)) => matches!(answer, Ok(true)),
                    futures::future::Either::Right(_) => false,
                };
                let verdict = if allowed { "allowed" } else { "denied" };
                stream::iter(vec![
                    Event::Text(format!("[debug] {verdict}: {rest}")),
                    Event::Done(Outcome::Ok),
                ])
            })
            .flatten();
            return Ok(Box::pin(stream::iter(vec![ask]).chain(outcome)));
        }
        let events = vec![
            Event::Text(format!("[debug] ran: {}", input.text)),
            Event::Done(Outcome::Ok),
        ];
        Ok(Box::pin(stream::iter(events)))
    }

    async fn insert(&self, input: Input) -> Result<()> {
        self.context.lock().expect("debug context lock").push(input);
        Ok(())
    }

    async fn grant(&self, pattern: &str) -> Result<()> {
        self.grants
            .lock()
            .expect("debug grants lock")
            .push(pattern.to_string());
        Ok(())
    }

    async fn revoke(&self, pattern: &str) -> Result<()> {
        self.grants
            .lock()
            .expect("debug grants lock")
            .retain(|p| p != pattern);
        Ok(())
    }

    async fn answer(&self, id: &str, allow: bool) -> Result<()> {
        let waiter = self
            .asks
            .lock()
            .expect("debug asks lock")
            .waiting
            .remove(id);
        match waiter {
            Some(tx) => {
                let _ = tx.send(allow); // a gone receiver means the turn ended: benign
                Ok(())
            }
            None => Err(ClatchError::NotFound(format!(
                "ask {id} (unknown or already resolved)"
            ))),
        }
    }

    async fn permissions(&self) -> Result<Vec<String>> {
        Ok(self.grants())
    }

    // The engine substrate persists model / instructions; a model-free echo
    // backend has nothing to apply them to, so the setters are honest no-ops.
    async fn set_model(&self, _model: Option<String>) -> Result<()> {
        Ok(())
    }

    async fn set_instructions(&self, _text: Option<String>) -> Result<()> {
        Ok(())
    }
}
