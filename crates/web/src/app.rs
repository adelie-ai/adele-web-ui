//! The Leptos component tree: a login screen, and the chat screen that wires the
//! reducer [`Engine`] to a live [`transport`] connection.
//!
//! Mobile-first and deliberately small — a single active conversation, a message
//! list, and a composer. The sidebar/conversation switcher, model & personality
//! pickers, KB, and tasks screens layer on top of the same engine next.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use futures::channel::mpsc;
use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use wasm_bindgen_futures::spawn_local;

use client_ui_common::UiMessage;

use crate::engine::{Engine, ViewSignals};
use crate::reauth::{AttemptOutcome, ReconnectAction};
use crate::settings::{self, SettingsPanel, SettingsSheet};
use crate::transport::ConnectError;
use crate::{auth, context, reauth, transport};

/// Root component. Shows the login screen until a token is present, then the
/// chat screen; signing out clears the token and returns here.
#[component]
pub fn App() -> impl IntoView {
    let session = RwSignal::new(auth::load_token());
    view! {
        <Show
            when=move || session.with(Option::is_some)
            fallback=move || view! { <LoginScreen session=session /> }
        >
            <ChatScreen session=session />
        </Show>
    }
}

/// Username/password form that exchanges credentials for a JWT and stores it in
/// `session` on success.
#[component]
fn LoginScreen(session: RwSignal<Option<String>>) -> impl IntoView {
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let pending = RwSignal::new(false);

    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if pending.get() {
            return;
        }
        let (user, pass) = (username.get(), password.get());
        pending.set(true);
        error.set(None);
        spawn_local(async move {
            match auth::login(&user, &pass).await {
                Ok(token) => session.set(Some(token)),
                Err(e) => error.set(Some(e)),
            }
            pending.set(false);
        });
    };

    view! {
        <main class="app-shell login">
            <h1>"Adele"</h1>
            <form class="login-form" on:submit=submit>
                <input
                    type="text"
                    placeholder="Username"
                    autocomplete="username"
                    prop:value=move || username.get()
                    on:input=move |ev| username.set(event_target_value(&ev))
                />
                <input
                    type="password"
                    placeholder="Password"
                    autocomplete="current-password"
                    prop:value=move || password.get()
                    on:input=move |ev| password.set(event_target_value(&ev))
                />
                <button type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Signing in…" } else { "Sign in" }}
                </button>
                {move || error.get().map(|e| view! { <p class="error">{e}</p> })}
            </form>
        </main>
    }
}

/// The chat screen: spins up the engine, drives a (re)connecting session, and
/// renders the active conversation + composer.
#[component]
fn ChatScreen(session: RwSignal<Option<String>>) -> impl IntoView {
    let view = ViewSignals::new();
    let token = session.get_untracked().unwrap_or_default();

    let (ui_tx, ui_rx) = mpsc::unbounded::<UiMessage>();
    let engine = Rc::new(RefCell::new(Engine::new(
        view,
        ui_tx.clone(),
        "web".to_string(),
    )));

    // Engine loop: drain UiMessages (transport events + RPC replies) and apply.
    spawn_local({
        let engine = engine.clone();
        let mut ui_rx = ui_rx;
        async move {
            while let Some(msg) = ui_rx.next().await {
                engine.borrow_mut().dispatch(msg);
            }
        }
    });

    // Session loop: connect, kick the initial load, and reconnect on drop with
    // capped backoff. Cancelled when the screen unmounts (sign-out).
    let cancelled = Arc::new(AtomicBool::new(false));
    on_cleanup({
        let cancelled = cancelled.clone();
        move || cancelled.store(true, Ordering::Relaxed)
    });
    spawn_local({
        let engine = engine.clone();
        let ui_tx = ui_tx.clone();
        let cancelled = cancelled.clone();
        async move {
            let ws_url = match transport::same_origin_ws_url() {
                Ok(url) => url,
                Err(e) => {
                    let _ = ui_tx.unbounded_send(UiMessage::Error(e));
                    return;
                }
            };
            // Returning to login = forget the (dead) token and flip the app to
            // unauthenticated, so the `<Show>` above swaps in `LoginScreen`. The
            // `session` signal is owned by `App`, so setting it here is safe even
            // as this `ChatScreen` unmounts; the loop breaks immediately after.
            let return_to_login = move || {
                auth::clear_token();
                session.set(None);
            };
            let mut backoff_ms = 500u32;
            let mut reject_streak = 0u32;
            while !cancelled.load(Ordering::Relaxed) {
                // Layer 1 (pre-emptive): never present a token we can already see
                // is expired — drop straight to login instead of a doomed connect.
                if auth::token_is_expired(&token) {
                    return_to_login();
                    break;
                }
                let outcome = match transport::connect(&ws_url, &token, ui_tx.clone()).await {
                    Ok(conn) => {
                        // The upgrade opened: the token was accepted and a real
                        // session begins. Run it to completion (socket drop).
                        engine.borrow_mut().set_transport(conn.transport.clone());
                        engine.borrow().start_initial_load();
                        let _ = conn.closed.await;
                        engine.borrow_mut().clear_transport();
                        if cancelled.load(Ordering::Relaxed) {
                            break;
                        }
                        AttemptOutcome::Opened
                    }
                    // Refused before opening: an auth-rejection candidate. Stay
                    // quiet (no per-attempt error toast) and let the streak decide.
                    Err(ConnectError::RejectedUpgrade) => AttemptOutcome::RejectedBeforeOpen,
                    // Still connecting past the cap: a connectivity stall — retry.
                    Err(ConnectError::Unreachable) => AttemptOutcome::NetworkError,
                    // Couldn't even build the socket: surface it once, then retry.
                    Err(ConnectError::Construct(e)) => {
                        let _ = ui_tx.unbounded_send(UiMessage::Error(format!("connect: {e}")));
                        AttemptOutcome::NetworkError
                    }
                };

                // Layer 2 (reactive): fold the outcome into the reject streak. A
                // healthy session resets it (and the backoff); repeated
                // refuse-before-open with no working session between drops to
                // login; a network stall just keeps retrying, forever, as before.
                let (next_streak, action) = reauth::on_attempt(reject_streak, outcome);
                reject_streak = next_streak;
                match action {
                    ReconnectAction::Reconnect => backoff_ms = 500,
                    ReconnectAction::Retry => {}
                    ReconnectAction::ReturnToLogin => {
                        return_to_login();
                        break;
                    }
                }
                gloo_timers::future::TimeoutFuture::new(backoff_ms).await;
                backoff_ms = backoff_ms.saturating_mul(2).min(10_000);
            }
        }
    });

    // The composer text is engine-owned (`view.composer`) so the reducer can push
    // into it — clearing on enqueue and loading a queued message back on recall
    // (feat/queue-messages) — rather than being a component-local signal the
    // engine can't reach.
    let on_send = {
        let engine = engine.clone();
        move |ev: SubmitEvent| {
            ev.prevent_default();
            let text = view.composer.get_untracked();
            if text.trim().is_empty() {
                return;
            }
            // Capture busy state before the dispatch: a submit never starts a
            // stream synchronously, so this is the pre-submit truth. While a reply
            // streams, the reducer QUEUES the text and clears the composer itself
            // via `SetComposerText`, so we must not clear here (and must not on the
            // stream-complete auto-flush, which never runs through this handler and
            // leaves a fresh draft untouched). Idle sends/flushes emit no
            // `SetComposerText`, so we clear the just-sent text here.
            let streaming = view.streaming_active.get_untracked();
            engine.borrow_mut().submit_prompt(text);
            if !streaming {
                view.composer.set(String::new());
            }
        }
    };

    // Up/down-arrow recall over the queue (feat/queue-messages). ArrowUp starts a
    // recall only from an empty composer (never clobber a fresh draft) and then
    // walks backward through the queued messages; while editing, ArrowUp/ArrowDown
    // step between queued items, and ArrowDown off the last one cancels back to a
    // fresh composer. The pure walk decisions live in `crate::queue`.
    let on_keydown = {
        let engine = engine.clone();
        move |ev: leptos::ev::KeyboardEvent| match ev.key().as_str() {
            "ArrowUp" => {
                let editing = view.editing_queued.get_untracked();
                let queue_len = view.queued.with_untracked(Vec::len);
                let composer_empty = view.composer.with_untracked(|c| c.trim().is_empty());
                // Not editing: only recall from an empty composer with a queue.
                // Editing: always walk (the composer holds the checked-out item,
                // not a fresh draft).
                if editing.is_none() && !(composer_empty && queue_len > 0) {
                    return;
                }
                match crate::queue::recall_up(editing, queue_len) {
                    crate::queue::RecallAction::Edit(i) => {
                        ev.prevent_default();
                        engine.borrow_mut().edit_queued(i);
                    }
                    crate::queue::RecallAction::Cancel => {
                        ev.prevent_default();
                        engine.borrow_mut().cancel_queued_edit();
                    }
                    // At the earliest item (or nothing to recall): while editing,
                    // swallow the key so it doesn't jump the caret; otherwise let
                    // the browser handle it.
                    crate::queue::RecallAction::None => {
                        if editing.is_some() {
                            ev.prevent_default();
                        }
                    }
                }
            }
            "ArrowDown" => {
                let editing = view.editing_queued.get_untracked();
                if editing.is_none() {
                    return;
                }
                let queue_len = view.queued.with_untracked(Vec::len);
                ev.prevent_default();
                match crate::queue::recall_down(editing, queue_len) {
                    crate::queue::RecallAction::Edit(i) => engine.borrow_mut().edit_queued(i),
                    crate::queue::RecallAction::Cancel => engine.borrow_mut().cancel_queued_edit(),
                    crate::queue::RecallAction::None => {}
                }
            }
            _ => {}
        }
    };

    // Settings drawer: `None` = closed, `Some(panel)` = open on that panel. The
    // gear opens the drawer root; the model pill jumps straight to the Model
    // panel — both hang off the same host so future panels are drop-in.
    let settings_open = RwSignal::new(None::<SettingsPanel>);
    let open_settings = move |_| settings_open.set(Some(SettingsPanel::default()));
    let open_model = move |_| settings_open.set(Some(SettingsPanel::Model));

    // A `Copy`, `Send` handle to the `!Send` engine, so the settings drawer's
    // reactive children can reach it (see `settings::EngineHandle`). The engine
    // loop / composer keep using the `Rc` directly (they live in `spawn_local`
    // tasks and top-level event handlers, which don't require `Send`).
    let engine_handle: settings::EngineHandle = StoredValue::new_local(engine.clone());

    // Fetch the tool-activity snapshot when the opt-in is on (#59), deduped so a
    // streamed turn doesn't spam GetMessages: `sync_view` re-sets
    // `current_conversation_id` on every dispatch (including every delta) with no
    // PartialEq guard, so we gate on a Memo whose value only changes on a real
    // switch, a new message (send / turn completion → `messages` grows), or a
    // toggle. The snapshot is cleared on switch/off so a slow fetch never renders
    // the previous conversation's tool output; `refresh_tool_activity` also drops
    // a reply that arrives after a switch.
    let tool_activity_trigger = Memo::new(move |_| {
        (
            view.show_tool_activity.get(),
            view.current_conversation_id.get(),
            view.messages.with(Vec::len),
        )
    });
    let last_tool_activity_cid = StoredValue::new_local(None::<String>);
    Effect::new(move |_| {
        let (on, cid, _len) = tool_activity_trigger.get();
        if !on {
            engine_handle.with_value(|e| e.borrow().clear_tool_activity());
            last_tool_activity_cid.set_value(None);
            return;
        }
        let Some(id) = cid else { return };
        let switched = last_tool_activity_cid.get_value().as_deref() != Some(id.as_str());
        last_tool_activity_cid.set_value(Some(id.clone()));
        engine_handle.with_value(|e| {
            let e = e.borrow();
            if switched {
                e.clear_tool_activity();
            }
            e.refresh_tool_activity(id);
        });
    });

    // Conversation switcher drawer (issue #12): `false` = closed. The list now
    // updates live from other-client changes (#15) — a `ConversationListChanged`
    // event drives the reducer to refetch and repaint the sidebar. This
    // load-on-open refetch is a cheap resync backstop (e.g. after a missed event
    // while the socket was down).
    let sidebar_open = RwSignal::new(false);
    let open_sidebar = move |_| {
        engine_handle.with_value(|e| e.borrow().refresh_conversation_list());
        sidebar_open.set(true);
    };

    // The toast is a transient view concern; dismissing it just clears the signal.
    let dismiss_toast = move |_| view.toast.set(None);

    view! {
        <main class="app-shell chat">
            <header class="chat-header">
                <button
                    class="icon-btn"
                    aria-label="Open conversations"
                    on:click=open_sidebar
                >
                    "\u{2630}"
                </button>
                <span class="title">
                    {move || {
                        let t = view.title.get();
                        if t.is_empty() { "Adele".to_string() } else { t }
                    }}
                </span>
                <Show when=move || view.model_picker_visible.get()>
                    <button
                        class="model-pill"
                        aria-label="Choose model for this conversation"
                        on:click=open_model
                    >
                        {move || {
                            settings::model_button_label(
                                &view.models.get(),
                                &view.active_model.get(),
                            )
                        }}
                    </button>
                </Show>
                <span class=move || {
                    if view.connected.get() { "dot online" } else { "dot offline" }
                }></span>
                // Read-aloud toggle (issue #18): speaks completed replies via the
                // browser's SpeechSynthesis. Self-hiding when the API is absent.
                {crate::read_aloud::read_aloud_toggle(view)}
                // Show-tool-activity toggle (issue #59): reveals Adele's tool
                // results inline (collapsed). Off by default, persisted per device.
                {crate::tool_activity::tool_activity_toggle(view)}
                <button class="icon-btn" aria-label="Open settings" on:click=open_settings>
                    "\u{2699}"
                </button>
            </header>

            <Show when=move || view.toast.get().is_some()>
                <div class="toast" role="status">
                    <span>{move || view.toast.get().unwrap_or_default()}</span>
                    <button class="icon-btn" aria-label="Dismiss" on:click=dismiss_toast>
                        "\u{2715}"
                    </button>
                </div>
            </Show>

            <section class="messages">
                // Bubbles (markdown, issue #48) with tool results interleaved as
                // collapsed rows when the opt-in is on (issue #59).
                {crate::tool_activity::transcript_view(view)}
                <Show when=move || view.streaming_active.get()>
                    <div class="msg assistant streaming">
                        {crate::markdown::streaming_body(view.streaming)}
                    </div>
                </Show>
            </section>

            // Context-window usage indicator (issue #14): unobtrusive, above the
            // composer; hidden until the active conversation reports a reading.
            {context::context_usage_bar(view)}

            // Queued-messages strip (feat/queue-messages): messages submitted
            // while Adele is busy queue here as chips (edit / remove) until the
            // reply finishes and the batch flushes as one turn. Hidden when empty.
            {crate::queue::queued_chips(engine_handle, view)}

            <form class="composer" on:submit=on_send>
                <input
                    type="text"
                    placeholder="Message Adele…"
                    autocomplete="off"
                    prop:value=move || view.composer.get()
                    on:input=move |ev| view.composer.set(event_target_value(&ev))
                    on:keydown=on_keydown
                />
                <button type="submit" disabled=move || !view.send_enabled.get()>
                    "Send"
                </button>
            </form>

            <SettingsSheet
                engine=engine_handle
                view=view
                open=settings_open
                session=session
            />

            {crate::sidebar::conversation_sidebar(engine_handle, view, sidebar_open)}
        </main>
    }
}
