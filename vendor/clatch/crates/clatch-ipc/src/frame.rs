//! Length-prefixed JSON framing (reference/protocol.md, section Framing): a
//! 4-byte big-endian length, then a UTF-8 JSON body. Generic over any byte stream
//! so the same codec serves both ends and every transport.
//!
//! POLICY IS PER CHANNEL ([`FrameLimits`]). The admin channel carries real agent
//! conversations, so its reader is ROBUST: one bad frame never kills a healthy
//! stream (an oversized frame is drained, a malformed body discarded, reading
//! continues); only a length beyond the hard ceiling is fatal. The control pipe
//! carries tiny messages between two Clatch ends, so its reader is FAIL-FAST
//! (reference/protocol.md § Framing): a zero-length, oversized, or unparseable
//! frame is a desync bug and the reader closes the connection instead of skipping.

use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, ErrorKind};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The largest frame a reader accepts. Same-user transports carrying real
/// agent conversations (a single timeline entry holds a whole long message),
/// so this is a guard against absurdity, not a message-size policy.
pub const MAX_FRAME: usize = 64 << 20; // 64 MiB

/// A claimed length beyond this is not an oversized message, it is a broken
/// stream (desynced framing / garbage): the connection dies.
const CORRUPT_LEN: usize = 1 << 30; // 1 GiB

/// Per-channel framing policy (reference/protocol.md § Framing). The reader's two
/// dials: the largest frame it accepts, and whether a bad frame is fatal.
#[derive(Debug, Clone, Copy)]
pub struct FrameLimits {
    /// The largest frame the reader accepts.
    pub max_frame: usize,
    /// Fail-fast: a bad frame (zero-length, oversized, or unparseable) closes the
    /// connection instead of being skipped. The control pipe sets this; the admin
    /// channel does not.
    pub fail_fast: bool,
}

impl FrameLimits {
    /// The control pipe: tiny messages, both ends Clatch, so a bad frame is a
    /// framing bug and the reader closes (reference/protocol.md § Framing).
    pub const fn control() -> Self {
        Self {
            max_frame: 1 << 20, // 1 MiB
            fail_fast: true,
        }
    }

    /// The admin channel (daemon <-> client/GUI): real agent conversation frames
    /// ride it, so a huge or unparseable one is skipped, never fatal.
    pub const fn admin() -> Self {
        Self {
            max_frame: MAX_FRAME,
            fail_fast: false,
        }
    }
}

/// Write one length-prefixed JSON frame and flush it. `max_frame` is the SAME
/// bound the receiving reader enforces (its channel's [`FrameLimits::max_frame`]).
/// Symmetric on purpose (R2, 2026-07-23): a fail-fast reader (the control pipe)
/// closes on an over-size frame, so an over-size frame must be rejected LOUDLY at
/// the source rather than written fine here and then silently killing the peer. The
/// caller learns its payload was too large instead of the peer just vanishing.
pub async fn write<W, T>(w: &mut W, msg: &T, max_frame: usize) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize + ?Sized,
{
    let body = serde_json::to_vec(msg).map_err(|e| io::Error::new(ErrorKind::InvalidData, e))?;
    if body.len() > max_frame {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!(
                "frame {} bytes exceeds the channel limit {max_frame}",
                body.len()
            ),
        ));
    }
    let len = u32::try_from(body.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "frame exceeds u32"))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await
}

/// Read the next DELIVERABLE frame under `limits`, or `None` at a clean end of
/// stream (the peer closed, so the instance is gone, reference/protocol.md
/// § Transport).
///
/// - **Fail-fast** (the control pipe): a zero-length, oversized, or unparseable
///   frame is a desync bug between two Clatch ends, so it returns an error and the
///   caller closes the connection. No skipping, no resync.
/// - **Robust** (the admin channel): an oversized frame (> `limits.max_frame`) is
///   drained and skipped, an unparseable body is skipped; both are logged and
///   reading continues (the framing stays intact). Only a length past the hard
///   ceiling (1 GiB) is a corrupt stream and errors.
pub async fn read<R, T>(r: &mut R, limits: FrameLimits) -> io::Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    loop {
        let mut len = [0u8; 4];
        match r.read_exact(&mut len).await {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let len = u32::from_be_bytes(len) as usize;

        if limits.fail_fast {
            // Both ends are Clatch, so a length outside the tiny-message range is a
            // framing bug and the stream is already desynced: close, never skip.
            if len == 0 || len > limits.max_frame {
                return Err(io::Error::new(
                    ErrorKind::InvalidData,
                    format!("control-pipe frame length {len} not credible: desync, closing"),
                ));
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).await?;
            return serde_json::from_slice(&body).map(Some).map_err(|e| {
                io::Error::new(ErrorKind::InvalidData, format!("control-pipe frame: {e}"))
            });
        }

        if len > CORRUPT_LEN {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "frame length not credible: corrupt stream",
            ));
        }
        if len > limits.max_frame {
            drain(r, len).await?;
            eprintln!("clatch-ipc: skipped an oversized frame ({len} bytes > max)");
            continue;
        }
        let mut body = vec![0u8; len];
        r.read_exact(&mut body).await?;
        match serde_json::from_slice(&body) {
            Ok(msg) => return Ok(Some(msg)),
            Err(e) => {
                // The frame was consumed whole: the stream is still framed
                // correctly, only this body was not ours to understand.
                eprintln!("clatch-ipc: skipped an unparseable frame ({len} bytes): {e}");
                continue;
            }
        }
    }
}

/// Consume and discard exactly `len` bytes (an oversized frame's body), in
/// bounded chunks so skipping never allocates the frame it refuses.
async fn drain<R: AsyncRead + Unpin>(r: &mut R, len: usize) -> io::Result<()> {
    let mut remaining = len;
    let mut buf = [0u8; 64 * 1024];
    while remaining > 0 {
        let take = remaining.min(buf.len());
        r.read_exact(&mut buf[..take]).await?;
        remaining -= take;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn round_trip_and_clean_eof() {
        let (mut a, mut b) = tokio::io::duplex(256);
        write(&mut a, &json!({"hello": 1}), MAX_FRAME)
            .await
            .unwrap();
        drop(a); // clean close after one frame
        let got: Option<serde_json::Value> = read(&mut b, FrameLimits::admin()).await.unwrap();
        assert_eq!(got, Some(json!({"hello": 1})));
        let eof: Option<serde_json::Value> = read(&mut b, FrameLimits::admin()).await.unwrap();
        assert_eq!(eof, None, "closed stream reads as None, not an error");
    }

    #[tokio::test]
    async fn a_multi_megabyte_message_round_trips() {
        // The long-message field bug: 1 MiB was the old ceiling and a real
        // agent message crossed it. Multi-MB frames are normal traffic now.
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let msg = json!({ "text": "x".repeat(3 << 20) });
        let expected = msg.clone();
        let writer = tokio::spawn(async move { write(&mut a, &msg, MAX_FRAME).await });
        let got: Option<serde_json::Value> = read(&mut b, FrameLimits::admin()).await.unwrap();
        writer.await.unwrap().unwrap();
        assert_eq!(got, Some(expected));
    }

    #[tokio::test]
    async fn oversized_and_malformed_frames_are_skipped_not_fatal() {
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let writer = tokio::spawn(async move {
            // 1) An oversized frame: a valid length prefix past MAX_FRAME,
            //    then exactly that many bytes. The reader must drain it.
            let huge = (MAX_FRAME + 8) as u32;
            a.write_all(&huge.to_be_bytes()).await.unwrap();
            let chunk = vec![0u8; 64 * 1024];
            let mut remaining = huge as usize;
            while remaining > 0 {
                let take = remaining.min(chunk.len());
                a.write_all(&chunk[..take]).await.unwrap();
                remaining -= take;
            }
            // 2) A well-framed but unparseable body.
            let garbage = b"not json at all";
            a.write_all(&(garbage.len() as u32).to_be_bytes())
                .await
                .unwrap();
            a.write_all(garbage).await.unwrap();
            // 3) The frame that must still arrive.
            write(&mut a, &json!({"alive": true}), MAX_FRAME)
                .await
                .unwrap();
        });
        let got: Option<serde_json::Value> = read(&mut b, FrameLimits::admin()).await.unwrap();
        writer.await.unwrap();
        assert_eq!(
            got,
            Some(json!({"alive": true})),
            "one bad frame must never kill the stream"
        );
    }

    #[tokio::test]
    async fn a_corrupt_length_is_fatal() {
        let (mut a, mut b) = tokio::io::duplex(256);
        // A length no honest peer writes: past the hard ceiling, the stream
        // itself is not credible (we are not looking at framing anymore).
        a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        let err = read::<_, serde_json::Value>(&mut b, FrameLimits::admin())
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn a_control_reader_fails_fast_on_a_bad_frame() {
        // The control pipe is fail-fast (reference/protocol.md § Framing): where
        // the admin reader would skip an oversized or unparseable frame, the
        // control reader treats it as a desync and errors, so the caller closes.
        let ctrl = FrameLimits::control();

        // Oversized for the control bound (but well under the admin bound): skipped
        // there, fatal here.
        let (mut a, mut b) = tokio::io::duplex(64 * 1024);
        let over = (ctrl.max_frame + 8) as u32;
        tokio::spawn(async move {
            a.write_all(&over.to_be_bytes()).await.unwrap();
            let chunk = vec![0u8; 64 * 1024];
            let mut remaining = over as usize;
            while remaining > 0 {
                let take = remaining.min(chunk.len());
                a.write_all(&chunk[..take]).await.unwrap();
                remaining -= take;
            }
        });
        let err = read::<_, serde_json::Value>(&mut b, ctrl)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData, "oversized is fatal");

        // A well-framed but unparseable body is also fatal, not skipped.
        let (mut a, mut b) = tokio::io::duplex(256);
        let garbage = b"not json";
        tokio::spawn(async move {
            a.write_all(&(garbage.len() as u32).to_be_bytes())
                .await
                .unwrap();
            a.write_all(garbage).await.unwrap();
        });
        let err = read::<_, serde_json::Value>(&mut b, ctrl)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData, "unparseable is fatal");
    }
}
