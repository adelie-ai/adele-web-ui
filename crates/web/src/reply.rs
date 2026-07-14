//! Request/reply timeout core for the WebSocket transport, kept timer-agnostic
//! so it compiles and unit-tests on the host target (like [`crate::wire`] and
//! [`crate::model`]).
//!
//! Every `send_command` registers a one-shot in the pending map and awaits it.
//! Without a bound, a reply that never arrives — a stalled daemon handler, a lost
//! frame, or a frame the read pump cannot deliver (an unparseable or non-text
//! frame is dropped without resolving its request) — hangs that future *forever*.
//! Because the initial load awaits its commands in sequence, one such stall
//! bricks the whole session: `Connected` never fires, conversations never load,
//! chat never works, and the model picker stays empty with Refresh unable to
//! recover. [`await_reply`] races the reply against a caller-supplied timeout so
//! an undelivered reply becomes an ordinary error instead of an infinite hang.

use std::future::Future;

use desktop_assistant_api_model::CommandResult;
use futures::channel::oneshot;
use futures::future::Either;

/// The result of awaiting a correlated reply. A [`TimedOut`](ReplyOutcome::TimedOut)
/// is kept distinct from a [`TransportClosed`](ReplyOutcome::TransportClosed): on
/// timeout the request is *still registered* in the pending map and the caller
/// must evict it, whereas on a closed transport the read pump has already cleared
/// the whole map.
#[derive(Debug)]
pub enum ReplyOutcome {
    /// The correlated reply arrived first (a command result or a daemon error).
    Reply(Result<CommandResult, String>),
    /// The timeout elapsed before any reply — the request must be evicted.
    TimedOut,
    /// The socket dropped (the one-shot sender was dropped) before replying.
    TransportClosed,
}

/// Race a pending reply against `timeout`. Timer-agnostic: the wasm transport
/// passes a `gloo_timers` timeout; host tests pass any `Future<Output = ()>`.
pub async fn await_reply(
    rx: oneshot::Receiver<Result<CommandResult, String>>,
    timeout: impl Future<Output = ()>,
) -> ReplyOutcome {
    futures::pin_mut!(timeout);
    match futures::future::select(rx, timeout).await {
        Either::Left((Ok(reply), _)) => ReplyOutcome::Reply(reply),
        Either::Left((Err(_canceled), _)) => ReplyOutcome::TransportClosed,
        Either::Right(((), _)) => ReplyOutcome::TimedOut,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::future::{Future, pending, ready};
    use std::pin::Pin;

    // A timeout that never fires, so the reply must win.
    fn never() -> Pin<Box<dyn Future<Output = ()>>> {
        Box::pin(pending())
    }
    // A timeout that fires immediately, so it must win against a pending reply.
    fn immediate() -> impl Future<Output = ()> {
        ready(())
    }

    #[test]
    fn reply_before_timeout_returns_the_reply() {
        let (tx, rx) = oneshot::channel();
        tx.send(Ok(CommandResult::Ack)).expect("receiver is live");
        match block_on(await_reply(rx, never())) {
            ReplyOutcome::Reply(Ok(CommandResult::Ack)) => {}
            other => panic!("expected the delivered Ack, got {other:?}"),
        }
    }

    #[test]
    fn daemon_error_reply_is_carried_through() {
        let (tx, rx) = oneshot::channel();
        tx.send(Err("conversation not found".to_string()))
            .expect("receiver is live");
        match block_on(await_reply(rx, never())) {
            ReplyOutcome::Reply(Err(e)) => assert_eq!(e, "conversation not found"),
            other => panic!("expected the daemon error, got {other:?}"),
        }
    }

    // The regression test for the hang: a reply that never arrives must resolve
    // as `TimedOut` rather than blocking forever. Before the timeout was added,
    // `send_command` awaited the one-shot directly, so this future never
    // completed and the whole sequential initial load stalled. `block_on`
    // returning at all is the assertion that the hang is gone.
    #[test]
    fn missing_reply_times_out_instead_of_hanging() {
        // Keep the sender alive (not dropped) so the one-shot stays *pending*,
        // modelling an in-flight request whose reply is never delivered.
        let (_tx, rx) = oneshot::channel::<Result<CommandResult, String>>();
        match block_on(await_reply(rx, immediate())) {
            ReplyOutcome::TimedOut => {}
            other => panic!("a never-delivered reply must time out, got {other:?}"),
        }
    }

    #[test]
    fn dropped_sender_reports_transport_closed() {
        let (tx, rx) = oneshot::channel::<Result<CommandResult, String>>();
        drop(tx); // socket closed before replying
        match block_on(await_reply(rx, never())) {
            ReplyOutcome::TransportClosed => {}
            other => panic!("a dropped sender must report a closed transport, got {other:?}"),
        }
    }
}
