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
use gloo_net::websocket::futures::WebSocket;
use wasm_bindgen_futures::spawn_local;

use crate::wire::event_to_ui_message;
use client_ui_common::UiMessage;

/// Sentinel subprotocol offered alongside the JWT so the BFF can tell the marker
/// from the token. Must match the server's `ws_auth::BEARER_SUBPROTOCOL`.
pub const BEARER_SUBPROTOCOL: &str = "adele.bearer";

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
        rx.await
            .map_err(|_| "transport closed before reply".to_string())?
    }
}

/// Open a socket to `ws_url`, presenting `token` via the auth subprotocol, and
/// spawn its read/write pumps. Incoming events are mapped and pushed onto
/// `ui_tx`; a `Disconnected` message and the `closed` one-shot both fire when
/// the socket ends.
pub fn connect(
    ws_url: &str,
    token: &str,
    ui_tx: mpsc::UnboundedSender<UiMessage>,
) -> Result<Connection, String> {
    let ws = WebSocket::open_with_protocols(ws_url, &[BEARER_SUBPROTOCOL, token])
        .map_err(|e| format!("open websocket: {e}"))?;
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
        let Ok(Message::Text(text)) = msg else {
            // Bytes frames are unused; a read error ends the socket.
            if msg.is_err() {
                break;
            }
            continue;
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
                // not a silent drop.
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
