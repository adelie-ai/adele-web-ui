//! The Leptos component tree. For the scaffold this is a placeholder shell; the
//! auth/login screen, conversation sidebar, and streaming chat view land on top
//! of the `client-ui-common` reducer in the following steps.

use leptos::prelude::*;

/// Root component, mounted onto `<body>` by `main`.
#[component]
pub fn App() -> impl IntoView {
    view! {
        <main class="app-shell">
            <h1>"Adele"</h1>
            <p class="muted">"Web client — sign-in and chat land next."</p>
        </main>
    }
}
