//! Forward the union of the browser sessions' subscribed conversations onto the
//! BFF's single upstream daemon connection (#35), closing the last gap in live
//! multi-client sync (#15).
//!
//! ## The gap this closes
//! The daemon gates per-turn fan-out by conversation subscription: it fans a
//! turn's events to every OTHER daemon connection that is *subscribed* to that
//! conversation. The BFF reaches the daemon over ONE shared connection. Until
//! that connection tells the daemon which conversations its browsers are
//! viewing, the daemon never fans a *native* client's turn (gtk / tui / voice)
//! to the BFF, and [`crate::relay`] — which routes whatever lands on the BFF
//! connection onward to the right browser sessions — has nothing to route. So a
//! turn typed in gtk never reaches a browser viewing the same conversation.
//!
//! ## Design
//! We keep, per browser session, the set of conversations it is viewing, and
//! forward their **union** to the daemon as `Command::SubscribeConversations`
//! (which is set-replace, so the full union each time is correct). A
//! conversation stays in the union while *any* session views it and drops out
//! only when the last viewer leaves — so one browser closing never unsubscribes
//! a conversation another browser is still watching.
//!
//! The per-session sets are fed by the daemon registry's change observer
//! ([`ConversationSubscriptions::set_change_observer`]): the BFF cannot see the
//! embedded dispatcher's per-session `set_subscriptions` / `unregister` calls
//! any other way. The observer is synchronous, so it only recomputes the union
//! ([`UnionTracker::apply`]) and pushes it onto a `watch`; a background
//! [`run_forwarder`] task debounces (a `watch` coalesces bursts to the newest
//! value), de-duplicates identical sets, and issues the command. On daemon
//! reconnect the union is re-sent, because the `Connector` supervisor replays
//! only client-tool registrations across a reconnect (#246) — not conversation
//! subscriptions — so the daemon forgets them when the socket drops.
//!
//! ## Trust boundary — single-tenant (#20 pending)
//! Every browser session authenticates the BFF's one daemon connection as the
//! same user, so the union is that single user's conversations only; there is no
//! cross-user mixing to guard here. Per-user scoping of the union awaits
//! multi-tenancy (#20), exactly as [`crate::relay`] notes for the reverse path.

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{mpsc, watch};

/// How many times [`run_forwarder`] re-attempts the reconnect re-send before
/// giving up (the next real subscription change will refresh it regardless).
const RECONNECT_RESEND_ATTEMPTS: u32 = 3;
/// Delay between reconnect re-send attempts — the `Connector` may still be
/// mid-reconnect on the first try, and `send_command` fails fast (not blocks)
/// while the socket is down.
const RECONNECT_RESEND_BACKOFF: Duration = Duration::from_millis(250);

/// Tracks, per browser session, the set of conversations it is viewing, and
/// computes their union: the set the BFF must subscribe its one daemon
/// connection to.
#[derive(Default)]
pub struct UnionTracker {
    /// session id -> conversations that session is viewing. A session with an
    /// empty set is dropped from the map, so a conversation is in the union iff
    /// some live session views it (and the map stays bounded by live viewers).
    per_session: Mutex<HashMap<String, BTreeSet<String>>>,
}

impl UnionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply a session's new subscribed set (an empty slice means the session
    /// unsubscribed from all, or disconnected) and return the recomputed union,
    /// sorted and de-duplicated so it is a stable, comparable value.
    pub fn apply(&self, session_id: &str, conversation_ids: &[String]) -> Vec<String> {
        let _ = (session_id, conversation_ids);
        todo!("spec: implemented in the following commit")
    }
}

/// Sends the union of subscribed conversations to the daemon. Abstracted behind
/// a trait so [`run_forwarder`]'s de-dupe / reconnect logic is unit-testable
/// without a live daemon connection.
#[async_trait]
pub trait SubscriptionSink: Send + Sync {
    /// Set-replace the BFF connection's subscribed conversations on the daemon.
    async fn send_union(&self, conversation_ids: Vec<String>) -> Result<()>;
}

/// Drain union updates and forward each *changed* set to the daemon, re-sending
/// the current union on daemon reconnect.
///
/// - `union_rx`: the latest union from the observer. A `watch`, so a burst of
///   subscription changes coalesces to the newest value (debounce). When every
///   sender is dropped (BFF shutting down) the loop ends.
/// - `reconnect_rx`: a `()` per daemon reconnect; forces a re-send even though
///   the union is unchanged, because the daemon dropped the subscription when
///   the socket closed.
pub async fn run_forwarder(
    sink: Arc<dyn SubscriptionSink>,
    union_rx: watch::Receiver<Vec<String>>,
    reconnect_rx: mpsc::UnboundedReceiver<()>,
) {
    let _ = (sink, union_rx, reconnect_rx);
    todo!("spec: implemented in the following commit")
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- UnionTracker ---------------------------------------------------------

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn apply_single_session_returns_its_sorted_set() {
        let t = UnionTracker::new();
        assert_eq!(t.apply("s1", &ids(&["c2", "c1"])), ids(&["c1", "c2"]));
    }

    #[test]
    fn apply_merges_and_dedupes_union_across_sessions() {
        let t = UnionTracker::new();
        t.apply("s1", &ids(&["c1", "c2"]));
        // s2 overlaps on c2 and adds c3.
        assert_eq!(
            t.apply("s2", &ids(&["c2", "c3"])),
            ids(&["c1", "c2", "c3"]),
            "the union merges both sessions and de-duplicates the shared id"
        );
    }

    #[test]
    fn apply_empty_set_removes_the_session() {
        let t = UnionTracker::new();
        t.apply("s1", &ids(&["c1"]));
        assert_eq!(
            t.apply("s1", &[]),
            Vec::<String>::new(),
            "an empty set drops the session's contribution entirely"
        );
    }

    #[test]
    fn conversation_stays_until_no_session_views_it() {
        // Two sessions view c1; one leaves — c1 must remain (the other still
        // views it); only when the last viewer leaves does c1 drop out. This is
        // the race the union guards: a session dropping while another views the
        // same conversation must not unsubscribe it.
        let t = UnionTracker::new();
        t.apply("s1", &ids(&["c1"]));
        t.apply("s2", &ids(&["c1"]));
        assert_eq!(t.apply("s1", &[]), ids(&["c1"]), "c1 stays: s2 still views it");
        assert_eq!(
            t.apply("s2", &[]),
            Vec::<String>::new(),
            "c1 drops only when the last viewer leaves"
        );
    }

    #[test]
    fn apply_set_replace_drops_dropped_conversations() {
        // set_subscriptions is wholesale set-replace: switching a session from c1
        // to c2 must remove c1 from the union (if no one else views it).
        let t = UnionTracker::new();
        t.apply("s1", &ids(&["c1"]));
        assert_eq!(t.apply("s1", &ids(&["c2"])), ids(&["c2"]));
    }

    #[test]
    fn union_is_sorted_and_deduplicated_within_a_session() {
        let t = UnionTracker::new();
        assert_eq!(
            t.apply("s1", &ids(&["c3", "c1", "c1", "c2"])),
            ids(&["c1", "c2", "c3"]),
        );
    }

    // --- run_forwarder --------------------------------------------------------

    /// A [`SubscriptionSink`] that records every `send_union` and signals each
    /// call on a channel (so tests observe forwards deterministically, without
    /// sleeping). Can be armed to fail the next N sends to exercise retry paths.
    struct RecordingSink {
        calls: Mutex<Vec<Vec<String>>>,
        tx: mpsc::UnboundedSender<Vec<String>>,
        fails_remaining: Mutex<usize>,
    }

    impl RecordingSink {
        fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Vec<String>>) {
            let (tx, rx) = mpsc::unbounded_channel();
            (
                Arc::new(Self {
                    calls: Mutex::new(Vec::new()),
                    tx,
                    fails_remaining: Mutex::new(0),
                }),
                rx,
            )
        }

        /// Fail the next `n` `send_union` calls before succeeding again.
        fn arm_failures(&self, n: usize) {
            *self.fails_remaining.lock().unwrap() = n;
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SubscriptionSink for RecordingSink {
        async fn send_union(&self, conversation_ids: Vec<String>) -> Result<()> {
            self.calls.lock().unwrap().push(conversation_ids.clone());
            let fail = {
                let mut f = self.fails_remaining.lock().unwrap();
                if *f > 0 {
                    *f -= 1;
                    true
                } else {
                    false
                }
            };
            // Signal AFTER recording so a waiting test sees even a failed attempt.
            let _ = self.tx.send(conversation_ids);
            if fail {
                anyhow::bail!("simulated send failure");
            }
            Ok(())
        }
    }

    async fn recv_send(rx: &mut mpsc::UnboundedReceiver<Vec<String>>) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("expected a send within 2s")
            .expect("sink channel open")
    }

    async fn assert_no_send(rx: &mut mpsc::UnboundedReceiver<Vec<String>>) {
        let r = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        assert!(r.is_err(), "expected no further send, got {r:?}");
    }

    #[tokio::test]
    async fn forwards_each_distinct_union() {
        let (sink, mut rx) = RecordingSink::new();
        let (union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (_rc_tx, rc_rx) = mpsc::unbounded_channel();
        let _h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        union_tx.send(ids(&["c1"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]));
        union_tx.send(ids(&["c1", "c2"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1", "c2"]));
    }

    #[tokio::test]
    async fn dedupes_identical_consecutive_unions() {
        let (sink, mut rx) = RecordingSink::new();
        let (union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (_rc_tx, rc_rx) = mpsc::unbounded_channel();
        let _h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        union_tx.send(ids(&["c1"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]));
        // Same union again (e.g. a second session subscribes to the same conv):
        // must NOT re-issue the identical command.
        union_tx.send(ids(&["c1"])).unwrap();
        assert_no_send(&mut rx).await;
    }

    #[tokio::test]
    async fn empty_union_after_nonempty_sends_unsubscribe() {
        let (sink, mut rx) = RecordingSink::new();
        let (union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (_rc_tx, rc_rx) = mpsc::unbounded_channel();
        let _h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        union_tx.send(ids(&["c1"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]));
        // Last viewer left: the empty union must be sent (unsubscribe).
        union_tx.send(vec![]).unwrap();
        assert_eq!(recv_send(&mut rx).await, Vec::<String>::new());
    }

    #[tokio::test]
    async fn startup_empty_union_is_not_sent() {
        // No browser has subscribed yet: the initial empty union must not spend a
        // command telling the daemon to unsubscribe from nothing.
        let (sink, mut rx) = RecordingSink::new();
        let (_union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (_rc_tx, rc_rx) = mpsc::unbounded_channel();
        let _h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        assert_no_send(&mut rx).await;
        assert!(sink.calls().is_empty(), "nothing should have been sent");
    }

    #[tokio::test]
    async fn reconnect_resends_current_union() {
        let (sink, mut rx) = RecordingSink::new();
        let (union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (rc_tx, rc_rx) = mpsc::unbounded_channel();
        let _h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        union_tx.send(ids(&["c1"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]));
        // Daemon reconnected and forgot our subscription: re-send the SAME union.
        rc_tx.send(()).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]));
    }

    #[tokio::test]
    async fn reconnect_resend_retries_across_a_transient_failure() {
        let (sink, mut rx) = RecordingSink::new();
        let (union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (rc_tx, rc_rx) = mpsc::unbounded_channel();
        let _h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        union_tx.send(ids(&["c1"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]));

        // The first re-send after reconnect fails (socket still mid-reconnect);
        // the retry must succeed.
        sink.arm_failures(1);
        rc_tx.send(()).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]), "failed first attempt");
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]), "retry succeeds");
    }

    #[tokio::test]
    async fn send_failure_is_retried_on_the_next_change() {
        // A failed send must not poison the de-dupe: the last-sent set stays
        // unchanged so the next update re-attempts, even the same union.
        let (sink, mut rx) = RecordingSink::new();
        sink.arm_failures(1);
        let (union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (_rc_tx, rc_rx) = mpsc::unbounded_channel();
        let _h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        union_tx.send(ids(&["c1"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]), "first attempt fails");
        // Same union pushed again: because the prior send failed, it re-attempts.
        union_tx.send(ids(&["c1"])).unwrap();
        assert_eq!(recv_send(&mut rx).await, ids(&["c1"]), "retry now succeeds");
    }

    #[tokio::test]
    async fn forwarder_stops_when_the_union_channel_closes() {
        let (sink, _rx) = RecordingSink::new();
        let (union_tx, union_rx) = watch::channel(Vec::<String>::new());
        let (_rc_tx, rc_rx) = mpsc::unbounded_channel();
        let h = tokio::spawn(run_forwarder(sink.clone(), union_rx, rc_rx));

        drop(union_tx); // BFF shutting down: all union senders gone.
        tokio::time::timeout(Duration::from_secs(2), h)
            .await
            .expect("forwarder must stop when the union channel closes")
            .expect("task joined");
    }
}
