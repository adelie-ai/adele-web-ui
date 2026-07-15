//! The single WebSocket transport to the BFF.
//!
//! Speaks `api-model`'s `WsRequest`/`WsFrame` JSON over a `gloo-net` socket.
//! Every `WsRequest` carries a client `id`; the BFF replies with exactly one
//! `WsFrame::Result`/`Error` bearing that `id` (true even for `SendMessage`,
//! which acks immediately and then streams). Id-less `WsFrame::Event` frames are
//! the live stream — mapped to `UiMessage`s and pushed onto the engine channel.
//!
//! Auth rides the handshake as a subprotocol: the browser can't set the
//! `Authorization` header on a WebSocket upgrade, so it offers
//! `[BEARER_SUBPROTOCOL, <jwt>]` and the BFF relays it (see the server's
//! `ws_auth`). Token correlation + the read/write pumps live here; reconnect is
//! driven by [`connect`]'s `closed` signal in the engine's session loop.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use desktop_assistant_api_model::{Command, CommandResult, WsFrame, WsRequest};
use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::Message;
use gloo_net::websocket::State;
use gloo_net::websocket::futures::WebSocket;
use gloo_timers::future::TimeoutFuture;
use wasm_bindgen_futures::spawn_local;

use crate::reply::{ReplyOutcome, await_reply};
use crate::wire::event_to_ui_message;
use client_ui_common::UiMessage;

/// Sentinel subprotocol offered alongside the JWT so the BFF can tell the marker
/// from the token. Must match the server's `ws_auth::BEARER_SUBPROTOCOL`.
pub const BEARER_SUBPROTOCOL: &str = "adele.bearer";

/// How long [`Transport::send_command`] waits for a correlated reply before
/// giving up. A generous backstop (not a tight SLA): normal replies land in
/// well under a second, but a cold model refresh can enumerate a remote provider
/// for several seconds, so this must clear that while still bounding a genuine
/// stall so it can't hang the session forever.
const REPLY_TIMEOUT_MS: u32 = 30_000;

/// Build the same-origin `/ws` URL (`wss` under TLS, else `ws`) from the
/// browser's current location. The SPA is served by the BFF (or proxied to it
/// in dev), so the socket shares the page's origin.
pub fn same_origin_ws_url() -> Result<String, String> {
    let loc = web_sys::window().ok_or("no window object")?.location();
    let proto = loc
        .protocol()
        .map_err(|_| "read location.protocol".to_string())?;
    let host = loc.host().map_err(|_| "read location.host".to_string())?;
    let scheme = if proto == "https:" { "wss" } else { "ws" };
    Ok(format!("{scheme}://{host}/ws"))
}

type Pending = Rc<RefCell<HashMap<String, oneshot::Sender<Result<CommandResult, String>>>>>;

/// A live connection: the request channel plus a one-shot that fires when the
/// socket closes (so the session loop can reconnect).
pub struct Connection {
    pub transport: Rc<Transport>,
    pub closed: oneshot::Receiver<()>,
}

/// Handle for issuing commands over the socket. Cheap to clone via `Rc`.
pub struct Transport {
    out_tx: mpsc::UnboundedSender<String>,
    pending: Pending,
    next_id: Cell<u64>,
}

impl Transport {
    /// Send a command and await its correlated reply. `Err` on a daemon-side
    /// command error or if the socket drops before replying.
    pub async fn send_command(&self, command: Command) -> Result<CommandResult, String> {
        let id = {
            let n = self.next_id.get();
            self.next_id.set(n.wrapping_add(1));
            format!("r{n}")
        };
        let (tx, rx) = oneshot::channel();
        self.pending.borrow_mut().insert(id.clone(), tx);

        let text = serde_json::to_string(&WsRequest {
            id: id.clone(),
            command,
        })
        .map_err(|e| format!("encode request: {e}"))?;
        if self.out_tx.unbounded_send(text).is_err() {
            self.pending.borrow_mut().remove(&id);
            return Err("transport closed".to_string());
        }

        // Bound the wait. Without this, a reply that never arrives — a stalled
        // daemon handler, a lost frame, or a frame the read pump can't deliver
        // (an unparseable or non-text frame is dropped without resolving its
        // request) — hangs this future *forever*. Because `start_initial_load`
        // awaits its commands in sequence, a single such stall would otherwise
        // brick the whole session: `Connected` never fires (the UI never goes
        // online), conversations never load, chat never works, and the model
        // picker stays empty with Refresh unable to recover. A timeout turns an
        // undelivered reply into an ordinary error, honoring the "model/purpose
        // load failures are non-fatal" contract the initial load documents.
        match await_reply(rx, TimeoutFuture::new(REPLY_TIMEOUT_MS)).await {
            ReplyOutcome::Reply(reply) => reply,
            ReplyOutcome::TransportClosed => Err("transport closed before reply".to_string()),
            ReplyOutcome::TimedOut => {
                // The request is still registered; evict it so a late reply is
                // ignored rather than resolving a dead one-shot.
                self.pending.borrow_mut().remove(&id);
                Err(format!(
                    "no reply from server within {}s",
                    REPLY_TIMEOUT_MS / 1000
                ))
            }
        }
    }
}

/// Why a connection attempt did not begin a live, authenticated session. The
/// session loop uses these to tell an auth refusal (drop to login after a few in
/// a row, see [`crate::reauth`]) from an ordinary connectivity problem (keep
/// retrying with backoff).
#[derive(Debug)]
pub enum ConnectError {
    /// The socket couldn't even be constructed (bad URL / browser refusal).
    Construct(String),
    /// The upgrade resolved to a close *before ever opening* — the BFF refused
    /// the token (expiry / key rotation / revocation) or is unreachable; the
    /// browser can't distinguish these. Repeated occurrences with no working
    /// session between them are treated as an auth rejection.
    RejectedUpgrade,
    /// The socket was still connecting past the upgrade timeout — a connectivity
    /// stall, not a refusal.
    Unreachable,
}

/// Poll interval while the `/ws` upgrade is resolving. The handshake either opens
/// or is refused within a network round-trip; briefly polling the ready-state is
/// simpler and more robust than reaching into gloo-net's internal waker, and adds
/// only a couple of frames of latency.
const UPGRADE_POLL_MS: u32 = 20;

/// Cap on waiting for the upgrade to resolve before calling it a connectivity
/// stall rather than a refusal. A reachable BFF accepts or refuses in well under
/// a second; a socket still CONNECTING past this is a network problem (retry),
/// not an auth one (drop to login).
const UPGRADE_TIMEOUT_MS: u32 = 8_000;

/// Await the upgrade leaving CONNECTING: `Ok(())` once the socket is OPEN, or a
/// [`ConnectError`] if it closed before opening (refusal) or never resolved
/// (stall). Crucially this returns **before** anything is written to the socket,
/// so a refused upgrade never triggers the browser's "WebSocket is already in
/// CLOSING or CLOSED state" warning that the old optimistic write-then-fail loop
/// produced on every doomed reconnect.
async fn await_upgrade(ws: &WebSocket) -> Result<(), ConnectError> {
    let mut waited = 0u32;
    loop {
        match ws.state() {
            State::Open => return Ok(()),
            State::Closing | State::Closed => return Err(ConnectError::RejectedUpgrade),
            State::Connecting => {
                if waited >= UPGRADE_TIMEOUT_MS {
                    return Err(ConnectError::Unreachable);
                }
                TimeoutFuture::new(UPGRADE_POLL_MS).await;
                waited = waited.saturating_add(UPGRADE_POLL_MS);
            }
        }
    }
}

/// Open a socket to `ws_url`, presenting `token` via the auth subprotocol, and —
/// **once the upgrade has actually opened** — spawn its read/write pumps.
/// Incoming events are mapped and pushed onto `ui_tx`; a `Disconnected` message
/// and the `closed` one-shot both fire when the socket ends. Returns a
/// [`ConnectError`] if the upgrade is refused or the server is unreachable,
/// without ever writing to (and thus without spamming) a dead socket.
pub async fn connect(
    ws_url: &str,
    token: &str,
    ui_tx: mpsc::UnboundedSender<UiMessage>,
) -> Result<Connection, ConnectError> {
    let ws = WebSocket::open_with_protocols(ws_url, &[BEARER_SUBPROTOCOL, token])
        .map_err(|e| ConnectError::Construct(e.to_string()))?;

    // Wait for the upgrade to open before splitting the socket or spawning the
    // pumps. A rejected token transitions CONNECTING -> CLOSED without opening;
    // returning here means we never queue a doomed send (no console spam) and the
    // session loop gets a clean refused-vs-opened signal to drive re-auth.
    await_upgrade(&ws).await?;

    let (write, read) = ws.split();

    let (out_tx, out_rx) = mpsc::unbounded::<String>();
    spawn_local(write_pump(write, out_rx));

    let pending: Pending = Rc::new(RefCell::new(HashMap::new()));
    let (closed_tx, closed) = oneshot::channel();
    spawn_local(read_pump(read, pending.clone(), ui_tx, closed_tx));

    Ok(Connection {
        transport: Rc::new(Transport {
            out_tx,
            pending,
            next_id: Cell::new(0),
        }),
        closed,
    })
}

/// Drain serialized requests to the socket until the channel or socket closes.
async fn write_pump(
    mut write: futures::stream::SplitSink<WebSocket, Message>,
    mut out_rx: mpsc::UnboundedReceiver<String>,
) {
    while let Some(text) = out_rx.next().await {
        if write.send(Message::Text(text)).await.is_err() {
            break;
        }
    }
}

/// Read frames until the socket closes: resolve pending requests by `id`, push
/// mapped events onto the engine channel, then signal disconnect both ways.
async fn read_pump(
    mut read: futures::stream::SplitStream<WebSocket>,
    pending: Pending,
    ui_tx: mpsc::UnboundedSender<UiMessage>,
    closed_tx: oneshot::Sender<()>,
) {
    while let Some(msg) = read.next().await {
        let text = match msg {
            Ok(Message::Text(text)) => text,
            // A proxy/ingress can reframe a text payload as binary; decode it as
            // UTF-8 rather than silently dropping it (a dropped Result frame would
            // otherwise strand its request until the send timeout). Non-UTF-8
            // bytes are genuinely unusable — skip them.
            Ok(Message::Bytes(bytes)) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => continue,
            },
            // A read error ends the socket.
            Err(_) => break,
        };
        match serde_json::from_str::<WsFrame>(&text) {
            Ok(WsFrame::Result { id, result }) => {
                if let Some(tx) = pending.borrow_mut().remove(&id) {
                    let _ = tx.send(Ok(result));
                }
            }
            Ok(WsFrame::Error { id, error }) => {
                if let Some(tx) = pending.borrow_mut().remove(&id) {
                    let _ = tx.send(Err(error));
                }
            }
            Ok(WsFrame::Event { event }) => {
                if let Some(ui) = event_to_ui_message(event) {
                    let _ = ui_tx.unbounded_send(ui);
                }
            }
            Err(e) => {
                // A frame we can't parse is a protocol mismatch worth surfacing,
                // not a silent drop. Note we can't correlate it to a pending
                // request (the id is inside the frame we failed to parse), so the
                // request it was meant to answer is unblocked by the per-request
                // timeout in `send_command`, not here.
                let _ =
                    ui_tx.unbounded_send(UiMessage::Error(format!("bad frame from server: {e}")));
            }
        }
    }
    // Socket ended: fail any still-pending requests (their senders drop), tell
    // the reducer, and release the session loop to reconnect.
    pending.borrow_mut().clear();
    let _ = ui_tx.unbounded_send(UiMessage::Disconnected {
        reason: "connection closed".to_string(),
    });
    let _ = closed_tx.send(());
}
