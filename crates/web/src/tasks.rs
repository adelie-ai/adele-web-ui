//! Background-tasks panel (issue #50): surface active + recent background tasks
//! with live progress/status.
//!
//! **Reducer-driven, mirrored into a signal.** The shared `client-ui-common`
//! reducer *does* model background tasks — an incoming `Event::Task*` maps (in
//! `wire.rs`) to a `UiMessage::Task*`, which the reducer turns into the
//! host-facing `Effect::Task*` family (`TasksReplaceAll` / `TaskStarted` /
//! `TaskProgress` / `TaskCompleted`), exactly as the GTK client's `TasksModel`
//! consumes them. The web engine mirrors those effects into
//! `ViewSignals::tasks`, so the panel is a plain reactive render of that signal —
//! no parallel state machine, no per-event wiring. The small list-mutation
//! helpers ([`upsert`] / [`apply_progress`]) are the web analogue of GTK's
//! `TasksModel` methods and live here, host-testable, next to the formatting.
//!
//! **Authoritative snapshot on open + on completion.** The engine fetches the
//! daemon's authoritative list (`ListBackgroundTasks { include_finished: true }`
//! — active *and* recent, most-recent-first) when the panel opens and again on a
//! `TaskCompleted` (whose reducer effect carries only the id, not the terminal
//! `Completed` / `Failed` / `Cancelled` status), so a finished task is shown with
//! its real terminal status and stays visible as "recent" rather than the GTK
//! panel's drop-on-complete behaviour.
//!
//! **Split like `knowledge.rs` / `scratchpad.rs`.** The pure formatting/model
//! helpers live at module top and unit-test on the host target; the Leptos panel
//! is a `#[cfg(target_arch = "wasm32")]` submodule that consumes *these* helpers,
//! so the tested logic and the rendered logic can't drift.

use desktop_assistant_api_model as api;

/// The empty-list invite, shared by the header summary and the panel's empty
/// state so the two never drift.
pub const EMPTY_TASKS: &str =
    "No background tasks yet — background agents, subagents, and maintenance runs show up here.";

/// How many tasks a snapshot reads at once. A single generous page keeps the
/// panel a simple list (no pagination in v1); the daemon caps its retained
/// finished tasks well below this.
pub const TASKS_LIMIT: u32 = 50;

/// CSS modifier class for a task's status dot. Mirrors the GTK panel's
/// `status_class_for` so the colour vocabulary matches across clients.
pub fn status_class(status: api::TaskStatus) -> &'static str {
    match status {
        api::TaskStatus::Pending => "task-dot-pending",
        api::TaskStatus::Running => "task-dot-running",
        api::TaskStatus::Completed => "task-dot-completed",
        api::TaskStatus::Failed => "task-dot-failed",
        api::TaskStatus::Cancelled => "task-dot-cancelled",
    }
}

/// Human-readable status, so the dot colour is never the only signal.
pub fn status_label(status: api::TaskStatus) -> &'static str {
    match status {
        api::TaskStatus::Pending => "Pending",
        api::TaskStatus::Running => "Running",
        api::TaskStatus::Completed => "Completed",
        api::TaskStatus::Failed => "Failed",
        api::TaskStatus::Cancelled => "Cancelled",
    }
}

/// Short label for what *kind* of work a task is, so a row says more than its
/// title. Mirrors the GTK panel's `kind_label_for`.
pub fn kind_label(kind: &api::TaskKind) -> &'static str {
    match kind {
        api::TaskKind::Conversation { .. } => "Chat",
        api::TaskKind::Subagent { .. } => "Subagent",
        api::TaskKind::Standalone { .. } => "Agent",
        api::TaskKind::Maintenance { .. } => "Maintenance",
    }
}

/// Whether a task is still in flight (`Pending`/`Running`) — drives the
/// "N active" count in the summary and (later) any cancel affordance.
pub fn is_active(status: api::TaskStatus) -> bool {
    matches!(status, api::TaskStatus::Pending | api::TaskStatus::Running)
}

/// Format the elapsed time between `started_at` and either `ended_at` (finished)
/// or `now_ms` (still running). Negative spans clamp to `0s` defensively so a
/// skewed daemon clock can't produce a nonsense row. Mirrors GTK's `format_age`.
pub fn format_age(started_at: i64, ended_at: Option<i64>, now_ms: i64) -> String {
    let end = ended_at.unwrap_or(now_ms);
    let mut secs = ((end - started_at) / 1000).max(0);
    if secs < 60 {
        return format!("{secs}s");
    }
    let mut mins = secs / 60;
    secs %= 60;
    if mins < 60 {
        return format!("{mins}m {secs}s");
    }
    let hours = mins / 60;
    mins %= 60;
    format!("{hours}h {mins}m")
}

/// A one-line summary for the panel header from the active/finished split. An
/// all-empty list is handled by the caller (it shows [`EMPTY_TASKS`]); this
/// covers the non-empty cases.
pub fn summary(active: usize, finished: usize) -> String {
    match (active, finished) {
        (0, 0) => EMPTY_TASKS.to_string(),
        (a, 0) => format!("{a} active"),
        (0, f) => format!("{f} recent"),
        (a, f) => format!("{a} active \u{00b7} {f} recent"),
    }
}

/// The header summary for a task list: the invite when empty, else the
/// active/finished split.
pub fn header_summary(tasks: &[api::TaskView]) -> String {
    if tasks.is_empty() {
        return EMPTY_TASKS.to_string();
    }
    let active = tasks.iter().filter(|t| is_active(t.status)).count();
    summary(active, tasks.len() - active)
}

/// Insert (or replace) a task in the live list. A freshly-started task goes to
/// the front so it's immediately visible; an update to a known task replaces it
/// in place (preserving its position). The web analogue of GTK's
/// `TasksModel::upsert`, applied to the engine's `tasks` signal on
/// `Effect::TaskStarted`.
pub fn upsert(list: &mut Vec<api::TaskView>, task: api::TaskView) {
    match list.iter_mut().find(|t| t.id == task.id) {
        Some(slot) => *slot = task,
        None => list.insert(0, task),
    }
}

/// Apply a progress-hint update to an existing task. A no-op for an unknown id
/// (the registry may have garbage-collected it, or the snapshot hasn't landed):
/// it must never introduce a phantom row. Mirrors GTK's `apply_progress`.
pub fn apply_progress(list: &mut [api::TaskView], id: &str, progress_hint: Option<String>) {
    if let Some(task) = list.iter_mut().find(|t| t.id.0 == id) {
        task.progress_hint = progress_hint;
    }
}

/// The Leptos tasks panel (issue #50). Re-exported from the wasm-only [`ui`]
/// submodule; `settings.rs` renders it as the `Tasks` panel body.
#[cfg(target_arch = "wasm32")]
pub use ui::tasks_panel;

#[cfg(target_arch = "wasm32")]
mod ui {
    //! Mobile-first Leptos view: a live list of background-task rows. Each row is
    //! a status dot + a kind badge + the title, with an optional progress hint
    //! and the status/age, so a glance says what's running and how it's going.
    //!
    //! The list is a plain reactive render of `view.tasks`, which the engine
    //! keeps fresh: a `refresh_tasks()` snapshot on open (and its Refresh button)
    //! seeds it, and the mirrored `Effect::Task*` family updates it live. Every
    //! text child is assistant/daemon-produced, so it renders as escaped plain
    //! text (Leptos escapes text children) — never `inner_html`.

    use leptos::prelude::*;

    use desktop_assistant_api_model::TaskView;

    use super::{EMPTY_TASKS, format_age, header_summary, kind_label, status_class, status_label};
    use crate::engine::ViewSignals;
    use crate::settings::EngineHandle;

    /// Current wall-clock in epoch ms, for row ages. Read per render, so ages
    /// refresh whenever the list changes (a task event / a snapshot); a running
    /// task's age is therefore approximate between events, which is fine for a
    /// glanceable panel.
    fn now_ms() -> i64 {
        js_sys::Date::now() as i64
    }

    /// The panel body. Loads the authoritative snapshot on open, then renders
    /// `view.tasks` live (the engine mirrors `Effect::Task*` into it).
    pub fn tasks_panel(engine: EngineHandle, view: ViewSignals) -> impl IntoView {
        // Load the snapshot once the panel mounts (re-created each open, so it
        // refreshes every time the tab opens). Deferred via an effect so the
        // signal writes don't happen during render.
        Effect::new(move |_| {
            engine.with_value(|e| e.borrow().refresh_tasks());
        });

        let refresh = move |_| engine.with_value(|e| e.borrow().refresh_tasks());

        view! {
            <section class="panel tasks-panel">
                <div class="panel-intro">
                    <p class="panel-summary">
                        {move || header_summary(&view.tasks.get())}
                    </p>
                    <p class="panel-note muted">
                        "Background agents, subagents, and maintenance runs Adele is working on \u{2014} \
                         updates live. Read-only here for now."
                    </p>
                </div>

                <div class="field-head">
                    <span class="field-label">"Tasks"</span>
                    <button class="link" on:click=refresh>
                        "Refresh"
                    </button>
                </div>

                {move || {
                    view.tasks_error
                        .get()
                        .map(|e| {
                            view! {
                                <p class="conn-error" role="alert">
                                    {e}
                                </p>
                            }
                                .into_any()
                        })
                        .unwrap_or_else(|| ().into_any())
                }}

                {move || {
                    let tasks = view.tasks.get();
                    if tasks.is_empty() {
                        empty_state(view.tasks_busy.get(), view.tasks_loaded.get())
                    } else {
                        let now = now_ms();
                        tasks
                            .into_iter()
                            .map(|task| task_row(task, now))
                            .collect_view()
                            .into_any()
                    }
                }}
            </section>
        }
    }

    /// The empty-list message, distinguishing an in-flight first load from a
    /// genuinely-empty list.
    fn empty_state(busy: bool, loaded: bool) -> AnyView {
        if busy && !loaded {
            view! { <p class="empty muted">"Loading\u{2026}"</p> }.into_any()
        } else {
            view! { <p class="empty muted">{EMPTY_TASKS}</p> }.into_any()
        }
    }

    /// One task row: a status dot, a dim kind badge + the title, an optional
    /// progress hint, and the status/age. Tap targets are display-only in v1.
    fn task_row(task: TaskView, now_ms: i64) -> AnyView {
        let dot_class = format!("task-dot {}", status_class(task.status));
        let kind = kind_label(&task.kind);
        let status = status_label(task.status);
        let age = format_age(task.started_at, task.ended_at, now_ms);
        let title = task.title.clone();
        let hint = task.progress_hint.clone();

        view! {
            <div class="task-row" class:active=super::is_active(task.status)>
                <span class=dot_class aria-hidden="true"></span>
                <div class="task-main">
                    <div class="task-title-row">
                        <span class="task-kind muted">{kind}</span>
                        <span class="task-title">{title}</span>
                    </div>
                    {hint
                        .filter(|h| !h.is_empty())
                        .map(|h| view! { <p class="task-hint muted">{h}</p> })}
                </div>
                <div class="task-meta">
                    <span class="task-status">{status}</span>
                    <span class="task-age muted">{age}</span>
                </div>
            </div>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use desktop_assistant_api_model as api;

    fn task(id: &str, status: api::TaskStatus) -> api::TaskView {
        api::TaskView {
            id: api::TaskId(id.into()),
            kind: api::TaskKind::Standalone {
                name: "agent".into(),
                conversation_id: "c1".into(),
            },
            status,
            started_at: 1_700_000_000_000,
            ended_at: None,
            last_error: None,
            parent: None,
            children: vec![],
            title: format!("Task {id}"),
            progress_hint: None,
        }
    }

    #[test]
    fn status_class_covers_every_status() {
        assert_eq!(status_class(api::TaskStatus::Pending), "task-dot-pending");
        assert_eq!(status_class(api::TaskStatus::Running), "task-dot-running");
        assert_eq!(
            status_class(api::TaskStatus::Completed),
            "task-dot-completed"
        );
        assert_eq!(status_class(api::TaskStatus::Failed), "task-dot-failed");
        assert_eq!(
            status_class(api::TaskStatus::Cancelled),
            "task-dot-cancelled"
        );
    }

    #[test]
    fn status_label_covers_every_status() {
        assert_eq!(status_label(api::TaskStatus::Pending), "Pending");
        assert_eq!(status_label(api::TaskStatus::Running), "Running");
        assert_eq!(status_label(api::TaskStatus::Completed), "Completed");
        assert_eq!(status_label(api::TaskStatus::Failed), "Failed");
        assert_eq!(status_label(api::TaskStatus::Cancelled), "Cancelled");
    }

    #[test]
    fn kind_label_covers_every_kind() {
        assert_eq!(
            kind_label(&api::TaskKind::Conversation {
                conversation_id: "c".into()
            }),
            "Chat"
        );
        assert_eq!(
            kind_label(&api::TaskKind::Subagent {
                parent_task_id: api::TaskId("p".into()),
                conversation_id: "c".into(),
                name: "child".into(),
            }),
            "Subagent"
        );
        assert_eq!(
            kind_label(&api::TaskKind::Standalone {
                name: "agent".into(),
                conversation_id: "c".into(),
            }),
            "Agent"
        );
        assert_eq!(
            kind_label(&api::TaskKind::Maintenance {
                name: "dream".into()
            }),
            "Maintenance"
        );
    }

    #[test]
    fn is_active_only_for_pending_and_running() {
        assert!(is_active(api::TaskStatus::Pending));
        assert!(is_active(api::TaskStatus::Running));
        assert!(!is_active(api::TaskStatus::Completed));
        assert!(!is_active(api::TaskStatus::Failed));
        assert!(!is_active(api::TaskStatus::Cancelled));
    }

    #[test]
    fn format_age_seconds_minutes_hours() {
        assert_eq!(format_age(0, None, 12_000), "12s");
        assert_eq!(format_age(0, None, 90_000), "1m 30s");
        assert_eq!(format_age(0, None, 3_725_000), "1h 2m");
    }

    #[test]
    fn format_age_uses_ended_at_when_finished_not_now() {
        // A finished task's age is frozen at ended_at, not the wall clock.
        let started = 1_000_000;
        let ended = started + 5_000;
        let now = started + 9_999_000; // far in the future
        assert_eq!(format_age(started, Some(ended), now), "5s");
    }

    #[test]
    fn format_age_clamps_negative_span_to_zero() {
        // Defensive: a skewed daemon clock (now < started) must not underflow.
        assert_eq!(format_age(1_000_000, None, 999_000), "0s");
    }

    #[test]
    fn summary_reports_active_and_recent_split() {
        assert_eq!(summary(2, 0), "2 active");
        assert_eq!(summary(0, 3), "3 recent");
        assert_eq!(summary(1, 4), "1 active \u{00b7} 4 recent");
    }

    #[test]
    fn header_summary_empty_invites() {
        assert_eq!(header_summary(&[]), EMPTY_TASKS);
    }

    #[test]
    fn header_summary_counts_active_vs_finished() {
        let tasks = vec![
            task("run", api::TaskStatus::Running),
            task("pend", api::TaskStatus::Pending),
            task("done", api::TaskStatus::Completed),
        ];
        // 2 active (running + pending), 1 finished (completed).
        assert_eq!(header_summary(&tasks), "2 active \u{00b7} 1 recent");
    }

    #[test]
    fn upsert_inserts_new_task_at_front() {
        let mut list = vec![task("older", api::TaskStatus::Running)];
        upsert(&mut list, task("newer", api::TaskStatus::Running));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id.0, "newer", "newest task goes to the front");
        assert_eq!(list[1].id.0, "older");
    }

    #[test]
    fn upsert_replaces_existing_task_in_place() {
        let mut list = vec![
            task("keep", api::TaskStatus::Running),
            task("update", api::TaskStatus::Running),
        ];
        let mut updated = task("update", api::TaskStatus::Completed);
        updated.title = "renamed".into();
        upsert(&mut list, updated);
        assert_eq!(list.len(), 2, "no new row for an existing id");
        assert_eq!(list[1].id.0, "update", "position is preserved");
        assert_eq!(list[1].status, api::TaskStatus::Completed);
        assert_eq!(list[1].title, "renamed");
    }

    #[test]
    fn apply_progress_updates_existing_task() {
        let mut list = vec![task("t1", api::TaskStatus::Running)];
        apply_progress(&mut list, "t1", Some("step 2/4".into()));
        assert_eq!(list[0].progress_hint.as_deref(), Some("step 2/4"));
        // Clearing the hint is honoured too.
        apply_progress(&mut list, "t1", None);
        assert_eq!(list[0].progress_hint, None);
    }

    #[test]
    fn apply_progress_for_unknown_id_is_a_noop() {
        // Unhappy path: a stray TaskProgress for an id we never saw must not
        // crash and must not introduce a phantom row.
        let mut list = vec![task("t1", api::TaskStatus::Running)];
        apply_progress(&mut list, "ghost", Some("hint".into()));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id.0, "t1");
        assert_eq!(list[0].progress_hint, None);
    }

    #[test]
    fn constants_are_sane() {
        assert!(TASKS_LIMIT >= 1, "must read at least one task");
        assert!(!EMPTY_TASKS.is_empty());
    }
}
