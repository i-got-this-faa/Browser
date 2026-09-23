//! The wake side-channel that makes the shell's frame pump event-driven.
//!
//! The perf audit showed the ~60Hz timer pump kept the process burning CPU
//! while idle. It is replaced by [`frame_wake`]: a one-slot async channel the
//! pump awaits. Producers on non-UI threads — the CEF frame sink, the CDP
//! reader thread, the config watcher, the control-socket acceptor — call
//! [`kick`]. Each kick deposits one permit and wakes the pump at most once
//! until it re-arms by awaiting again, so bursty producers (an animated page
//! paints hundreds of times per second) coalesce into one pump iteration per
//! rendered frame for free. An idle page kicks nothing at all.

use std::sync::OnceLock;

static WAKE: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> = OnceLock::new();

/// The receiving end the frame pump awaits. Idempotent: every caller gets the
/// same receiver of the same one-slot channel. Capacity 1 is the coalescing
/// buffer — it saturates instead of queuing a storm.
pub fn frame_wake() -> async_channel::Receiver<()> {
    WAKE.get_or_init(|| async_channel::bounded::<()>(1)).1.clone()
}

/// Nudge the frame pump from any thread. Never blocks: if the slot already
/// holds an unconsumed permit this is a no-op. Safe to call before
/// [`frame_wake`] is first awaited (or at all).
pub fn kick() {
    let (tx, _) = WAKE.get_or_init(|| async_channel::bounded::<()>(1));
    let _ = tx.try_send(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kick_coalesces_into_one_permit() {
        // Never awaited: the slot must saturate, not queue.
        for _ in 0..1000 {
            kick();
        }
        // First recv succeeds immediately (permit is present)...
        let rx = frame_wake();
        // ...and the very next one would block; verify non-blocking drain.
        assert!(matches!(
            rx.try_recv(),
            Ok(()) | Err(async_channel::TryRecvError::Empty)
        ));
        // After draining the single slot there is at most one more permit
        // buffered by racing kickers — but never a thousand.
        let mut extra = 0;
        while matches!(rx.try_recv(), Ok(())) {
            extra += 1;
            assert!(extra < 3, "permits queued: coalescing broken");
        }
    }
}
