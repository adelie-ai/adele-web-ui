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
use crate::{auth, transport};

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
            let mut backoff_ms = 500u32;
            while !cancelled.load(Ordering::Relaxed) {
                match transport::connect(&ws_url, &token, ui_tx.clone()) {
                    Ok(conn) => {
                        backoff_ms = 500;
                        engine.borrow_mut().set_transport(conn.transport.clone());
                        engine.borrow().start_initial_load();
                        let _ = conn.closed.await;
                        engine.borrow_mut().clear_transport();
                        if cancelled.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = ui_tx.unbounded_send(UiMessage::Error(format!("connect: {e}")));
                    }
                }
                gloo_timers::future::TimeoutFuture::new(backoff_ms).await;
                backoff_ms = backoff_ms.saturating_mul(2).min(10_000);
            }
        }
    });

    let draft = RwSignal::new(String::new());
    let on_send = {
        let engine = engine.clone();
        move |ev: SubmitEvent| {
            ev.prevent_default();
            let text = draft.get();
            if text.trim().is_empty() {
                return;
            }
            engine.borrow_mut().submit_prompt(text);
            draft.set(String::new());
        }
    };

    let sign_out = move |_| {
        auth::clear_token();
        session.set(None);
    };

    view! {
        <main class="app-shell chat">
            <header class="chat-header">
                <span class="title">
                    {move || {
                        let t = view.title.get();
                        if t.is_empty() { "Adele".to_string() } else { t }
                    }}
                </span>
                <span class=move || {
                    if view.connected.get() { "dot online" } else { "dot offline" }
                }></span>
                <button class="link" on:click=sign_out>"Sign out"</button>
            </header>

            <section class="messages">
                {move || {
                    view.messages
                        .get()
                        .into_iter()
                        .map(|m| {
                            view! { <div class=format!("msg {}", m.role)><p>{m.content}</p></div> }
                        })
                        .collect_view()
                }}
                <Show when=move || view.streaming_active.get()>
                    <div class="msg assistant streaming">
                        <p>{move || view.streaming.get()}</p>
                    </div>
                </Show>
            </section>

            <form class="composer" on:submit=on_send>
                <input
                    type="text"
                    placeholder="Message Adele…"
                    autocomplete="off"
                    prop:value=move || draft.get()
                    on:input=move |ev| draft.set(event_target_value(&ev))
                />
                <button type="submit" disabled=move || !view.send_enabled.get()>
                    "Send"
                </button>
            </form>
        </main>
    }
}
