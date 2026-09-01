//! The shared JSON-RPC pump. One background task owns the split stream and
//! multiplexes it: outbound requests/notifications/responses arrive as commands,
//! inbound frames are sorted by [`Frame::classify`](crate::Frame::classify) into
//! reply waiters (matched by id), an inbound-request channel, and a notification
//! channel. Every vocabulary built on this crate drives its connection through a
//! `Peer`.

use crate::frame::{self, FrameLimits};
use crate::rpc::{Frame, Id, Kind, Notification, Request, Response};
use clatch_core::{ClatchError, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};

/// What the pump should put on the wire.
enum Cmd {
    Request {
        msg: Request,
        reply: oneshot::Sender<Response>,
    },
    Notify(Notification),
    Respond(Response),
    /// Respond, then ack once the bytes hit the socket (shutdown replies).
    RespondFlushed(Response, oneshot::Sender<()>),
    /// Drop a pending request whose caller gave up (timed out), so its slot in
    /// the pump's `pending` map is reclaimed instead of leaking until the
    /// connection closes.
    Forget(Id),
}

/// A handle to a running connection. Cloneable senders sit behind it; dropping
/// every `Peer` closes the command channel, which ends the pump.
pub struct Peer {
    cmd: mpsc::Sender<Cmd>,
    next_id: AtomicU64,
    addr: String,
}

/// The inbound halves a consumer reads from: callbacks it must answer, and
/// fire-and-forget notifications.
pub struct Inbox {
    pub requests: mpsc::Receiver<Request>,
    pub notifications: mpsc::Receiver<Notification>,
}

impl Peer {
    /// Start the pump over a connected stream, reading under `limits` (the control
    /// pipe is fail-fast, the admin channel is robust; reference/protocol.md
    /// § Framing). Returns the handle and the inbound channels.
    pub fn start<S>(stream: S, addr: String, limits: FrameLimits) -> (Peer, Inbox)
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>(64);
        let (req_tx, req_rx) = mpsc::channel::<Request>(64);
        let (notif_tx, notif_rx) = mpsc::channel::<Notification>(64);
        tokio::spawn(pump(stream, addr.clone(), limits, cmd_rx, req_tx, notif_tx));
        let peer = Peer {
            cmd: cmd_tx,
            next_id: AtomicU64::new(1),
            addr,
        };
        (
            peer,
            Inbox {
                requests: req_rx,
                notifications: notif_rx,
            },
        )
    }

    /// Send a request and await its reply, bounded by `timeout`.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Response> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        // EVERY wait carries the timeout: the send itself can block if the pump
        // is parked on a stuck socket write and the command channel is full, so
        // enqueuing the request is bounded too (a hung peer must never freeze a
        // caller - e.g. a shutdown request in the supervisor path, 2026-07-19).
        let enqueue = self.cmd.send(Cmd::Request {
            msg: Request::new(id, method, params),
            reply: tx,
        });
        match tokio::time::timeout(timeout, enqueue).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return Err(self.closed()),
            Err(_) => {
                return Err(ClatchError::Invalid(format!(
                    "pipe {}: {method} send timed out",
                    self.addr
                )))
            }
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(_)) => Err(self.closed()),
            Err(_) => {
                // We gave up: tell the pump to forget this id so its `pending`
                // slot is not held for the life of the connection. Best-effort:
                // if the pump is already gone the map dies with it.
                let _ = self.cmd.send(Cmd::Forget(id)).await;
                Err(ClatchError::Invalid(format!(
                    "pipe {}: {method} timed out",
                    self.addr
                )))
            }
        }
    }

    /// Send a fire-and-forget notification.
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.cmd
            .send(Cmd::Notify(Notification::new(method, params)))
            .await
            .map_err(|_| self.closed())
    }

    /// Fire a notification WITHOUT blocking: if the command channel is full (the
    /// peer is not draining its socket) the notification is dropped, never awaited.
    /// For pushes the caller must never park on (R1, 2026-07-23): the
    /// connected-agents roster (full-snapshot-on-change) and refusal notices
    /// (advisory) are safe to drop under backpressure, and a supervisor that
    /// awaited them here could wedge the whole daemon.
    pub fn notify_lossy(&self, method: &str, params: Value) {
        let _ = self
            .cmd
            .try_send(Cmd::Notify(Notification::new(method, params)));
    }

    /// Answer an inbound request.
    pub async fn respond(&self, resp: Response) -> Result<()> {
        self.cmd
            .send(Cmd::Respond(resp))
            .await
            .map_err(|_| self.closed())
    }

    /// [`Self::respond`], returning only after the bytes are written to the
    /// socket: for replies that must reach the wire before the process state
    /// changes (a shutdown answer).
    pub async fn respond_flushed(&self, resp: Response) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(Cmd::RespondFlushed(resp, tx))
            .await
            .map_err(|_| self.closed())?;
        rx.await.map_err(|_| self.closed())
    }

    fn closed(&self) -> ClatchError {
        ClatchError::io(
            &self.addr,
            std::io::Error::new(ErrorKind::BrokenPipe, "control pipe closed"),
        )
    }
}

/// The pump loop: one task owning the write half and the command traffic, fed
/// inbound frames by a dedicated read task.
///
/// The read MUST live in its own task, never as a `select!` arm: `frame::read`
/// awaits twice (length, then body), so cancelling it mid-frame loses the bytes
/// it already consumed and desyncs the stream. In a select, any outbound command
/// landing while a frame is partially read did exactly that, and the fail-fast
/// reader then killed a healthy connection - rarely on unix (a small frame is
/// usually buffered whole, so the read never parks mid-frame), routinely on
/// Windows named pipes (the first observed casualty: an app's signal burst
/// racing the daemon's connected-agents push, reference/cross-platform.md B6).
/// A channel `recv` is cancellation-safe; a multi-await read is not.
async fn pump<S>(
    stream: S,
    addr: String,
    limits: FrameLimits,
    mut cmd: mpsc::Receiver<Cmd>,
    requests: mpsc::Sender<Request>,
    notifications: mpsc::Sender<Notification>,
) where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut rd, mut wr) = tokio::io::split(stream);
    let (in_tx, mut in_rx) = mpsc::channel::<Frame>(64);
    let reader = tokio::spawn(async move {
        loop {
            match frame::read::<_, Frame>(&mut rd, limits).await {
                Ok(Some(f)) => {
                    if in_tx.send(f).await.is_err() {
                        return; // the pump ended; nobody wants the rest
                    }
                }
                Ok(None) => return, // clean EOF: the peer closed
                Err(e) => {
                    // A broken stream ends the connection like an EOF, but never
                    // silently: a desync here is a bug worth a trace.
                    eprintln!("clatch-ipc: {addr}: read failed: {e}");
                    return;
                }
            }
        }
    });
    let mut pending: HashMap<Id, oneshot::Sender<Response>> = HashMap::new();
    loop {
        tokio::select! {
            inbound = in_rx.recv() => {
                let Some(frame) = inbound else {
                    break; // the read task ended: EOF or a broken stream
                };
                // A `send` fails only when the consumer dropped its receiver; that
                // is a reason to stop the pump.
                let forwarded = match frame.classify() {
                    Some(Kind::Response(r)) => {
                        if let Some(tx) = pending.remove(&r.id) {
                            let _ = tx.send(r);
                        }
                        Ok(())
                    }
                    Some(Kind::Request(r)) => requests.send(r).await.map_err(|_| ()),
                    Some(Kind::Notification(n)) => notifications.send(n).await.map_err(|_| ()),
                    None => Ok(()), // malformed: drop it, keep the connection
                };
                if forwarded.is_err() {
                    break;
                }
            }
            outbound = cmd.recv() => {
                let written = match outbound {
                    Some(Cmd::Request { msg, reply }) => {
                        pending.insert(msg.id, reply);
                        frame::write(&mut wr, &msg, limits.max_frame).await
                    }
                    Some(Cmd::Notify(msg)) => frame::write(&mut wr, &msg, limits.max_frame).await,
                    Some(Cmd::Respond(msg)) => frame::write(&mut wr, &msg, limits.max_frame).await,
                    Some(Cmd::RespondFlushed(msg, ack)) => {
                        let written = frame::write(&mut wr, &msg, limits.max_frame).await;
                        if written.is_ok() {
                            let _ = ack.send(());
                        }
                        written
                    }
                    Some(Cmd::Forget(id)) => {
                        pending.remove(&id); // a timed-out caller reclaimed its slot
                        Ok(())
                    }
                    None => break, // every Peer handle dropped
                };
                if written.is_err() {
                    break;
                }
            }
        }
    }
    // The read task parks in `frame::read` on a quiet stream and would outlive
    // us holding the read half; it shares no state, so an abort is clean.
    reader.abort();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Inbound frames survive concurrent outbound traffic. The regression this
    /// pins (Windows B6, reference/cross-platform.md): when the read was a
    /// `select!` arm, an outbound command landing mid-frame cancelled it, the
    /// consumed bytes vanished, and the desynced stream died silently. The tiny
    /// duplex buffer forces every read to park partway through a frame while
    /// the outbound side floods, so the old shape fails here on every OS.
    #[tokio::test]
    async fn inbound_frames_survive_concurrent_outbound_traffic() {
        let n = 200usize;
        let (local, remote) = tokio::io::duplex(8);
        let (peer, mut inbox) = Peer::start(local, "test".into(), FrameLimits::control());

        // The remote end: flood n notifications inward, drain our outbound
        // (the tiny buffer would otherwise fill and park both sides).
        let (mut r_rd, mut r_wr) = tokio::io::split(remote);
        let flood = tokio::spawn(async move {
            for i in 0..n {
                frame::write(
                    &mut r_wr,
                    &Notification::new("sig", json!({ "i": i })),
                    FrameLimits::control().max_frame,
                )
                .await
                .unwrap();
            }
            r_wr // keep the write half open until the test ends
        });
        let drain = tokio::spawn(async move {
            while let Ok(Some(_)) = frame::read::<_, Frame>(&mut r_rd, FrameLimits::control()).await
            {
            }
        });

        // Collect the inbound flood while this side floods outbound.
        let collect = tokio::spawn(async move {
            let mut got = Vec::new();
            while got.len() < n {
                match tokio::time::timeout(Duration::from_secs(10), inbox.notifications.recv())
                    .await
                {
                    Ok(Some(x)) => got.push(x),
                    _ => break, // timeout or closed: the assert below reports it
                }
            }
            got
        });
        for i in 0..n {
            peer.notify("out", json!({ "i": i })).await.unwrap();
        }

        let got = collect.await.unwrap();
        assert_eq!(got.len(), n, "every inbound frame arrives, none desynced");
        for (i, g) in got.iter().enumerate() {
            assert_eq!(g.params["i"], i, "in order, none lost");
        }
        let _wr = flood.await.unwrap();
        drain.abort();
    }
}
