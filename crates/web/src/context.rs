//! Context-window usage indicator (issue #14).
//!
//! The token counts, percentage, colour bucketing, and the glanceable
//! `12k / 32k (38%)` readout all live in the shared reducer's
//! [`ContextUsageView`] (client-ui-common, DA#341) — the SPA *surfaces* that
//! view rather than recomputing a budget, so it can never disagree with the
//! other clients. This module adds only the two web-specific presentation
//! concerns the shared model doesn't own: a spelled-out screen-reader label and
//! the clamped progress-bar fill width. Both are kept transport-/view-free so
//! they unit-test on the host target like [`crate::model`] / [`crate::wire`];
//! the `#[cfg(target_arch = "wasm32")]` Leptos view lives at the bottom.

use client_ui_common::ContextUsageView;

/// Accessible description of the current fill, spelled out for assistive tech.
///
/// The visible pill shows the shared [`ContextUsageView::readout`]
/// (`12k / 32k (38%) ⟳`), whose `/` and `⟳` read poorly aloud; this expands the
/// same figures into words — `"Context window 38% full, 12000 of 32000 tokens"`
/// — appending `", compacting"` while a windowing/compaction pass is active.
/// The percentage comes from the shared [`ContextUsageView::fraction`] so the
/// spoken and shown numbers can never drift apart.
pub fn aria_label(usage: &ContextUsageView) -> String {
    let pct = (usage.fraction() * 100.0).round() as u64;
    let mut label = format!(
        "Context window {pct}% full, {} of {} tokens",
        usage.used_tokens, usage.budget_tokens
    );
    if usage.compaction_active {
        label.push_str(", compacting");
    }
    label
}

/// Fill width for the progress bar, an integer percent clamped to `0..=100`.
///
/// The shared [`ContextUsageView::fraction`] can exceed `1.0` on overflow (used
/// past budget); the bar caps at full so it never paints past its track — the
/// red colour bucket ([`ContextFillLevel::Red`](client_ui_common::ContextFillLevel))
/// is what signals the overflow, not a bar wider than 100%.
pub fn bar_percent(usage: &ContextUsageView) -> u8 {
    (usage.fraction() * 100.0).round().clamp(0.0, 100.0) as u8
}

#[cfg(target_arch = "wasm32")]
pub use view::context_usage_bar;

#[cfg(target_arch = "wasm32")]
mod view {
    use leptos::prelude::*;

    use super::{aria_label, bar_percent};
    use crate::engine::ViewSignals;

    /// Slim context-window fill indicator, shown just above the composer once a
    /// turn on the viewed conversation reports usage. It stays hidden (zero
    /// footprint) before the first turn and right after a conversation switch,
    /// so it never crowds the phone chat header. The readout, percentage, and
    /// colour bucket all come from the shared [`ContextUsageView`]
    /// (client-ui-common, DA#341) — this only lays them out. `role="status"` +
    /// `aria-live="polite"` announce the fill to assistive tech as it updates
    /// each turn, using the spelled-out [`super::aria_label`] rather than the
    /// symbol-laden visible readout.
    ///
    /// [`ContextUsageView`]: client_ui_common::ContextUsageView
    pub fn context_usage_bar(view: ViewSignals) -> impl IntoView {
        move || {
            view.context_usage.get().map(|usage| {
                let label = aria_label(&usage);
                view! {
                    <div
                        class=format!("context-usage {}", usage.level().css_class())
                        role="status"
                        aria-live="polite"
                        aria-label=label.clone()
                        title=label
                    >
                        <div class="context-bar">
                            <div
                                class="context-bar-fill"
                                style=format!("width:{}%", bar_percent(&usage))
                            ></div>
                        </div>
                        <span class="context-readout">{usage.readout()}</span>
                    </div>
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(used: u64, budget: u64, compaction: bool) -> ContextUsageView {
        ContextUsageView {
            used_tokens: used,
            budget_tokens: budget,
            compaction_active: compaction,
        }
    }

    #[test]
    fn aria_label_spells_out_used_budget_and_percent() {
        // 12000 / 32000 == 0.375 -> 38% (matches the shared readout's "38%").
        assert_eq!(
            aria_label(&usage(12_000, 32_000, false)),
            "Context window 38% full, 12000 of 32000 tokens"
        );
    }

    #[test]
    fn aria_label_marks_active_compaction() {
        assert_eq!(
            aria_label(&usage(30_000, 32_000, true)),
            "Context window 94% full, 30000 of 32000 tokens, compacting"
        );
    }

    #[test]
    fn aria_label_omits_compaction_when_inactive() {
        assert!(!aria_label(&usage(30_000, 32_000, false)).contains("compacting"));
    }

    #[test]
    fn aria_label_reports_overflow_percent_verbatim() {
        // Over budget: the label reports the true (>100%) figure, unlike the
        // clamped bar — the words must not hide an overflow.
        assert_eq!(
            aria_label(&usage(40_000, 32_000, false)),
            "Context window 125% full, 40000 of 32000 tokens"
        );
    }

    #[test]
    fn aria_label_zero_budget_is_zero_percent_without_panic() {
        // Unknown/zero budget renders neutrally (fraction() == 0.0), never a
        // divide-by-zero.
        assert_eq!(
            aria_label(&usage(5_000, 0, false)),
            "Context window 0% full, 5000 of 0 tokens"
        );
    }

    #[test]
    fn bar_percent_rounds_fraction() {
        assert_eq!(bar_percent(&usage(12_000, 32_000, false)), 38);
        assert_eq!(bar_percent(&usage(500, 8_000, false)), 6);
    }

    #[test]
    fn bar_percent_zero_used_is_zero() {
        assert_eq!(bar_percent(&usage(0, 32_000, false)), 0);
    }

    #[test]
    fn bar_percent_full_is_100() {
        assert_eq!(bar_percent(&usage(32_000, 32_000, false)), 100);
    }

    #[test]
    fn bar_percent_clamps_overflow_to_100() {
        // 40000 / 32000 == 125%, but the bar must not exceed its track.
        assert_eq!(bar_percent(&usage(40_000, 32_000, false)), 100);
    }

    #[test]
    fn bar_percent_zero_budget_is_zero_without_panic() {
        assert_eq!(bar_percent(&usage(5_000, 0, false)), 0);
    }
}
