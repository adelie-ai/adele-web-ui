//! Test-only tracing capture.
//!
//! A [`Recorder`] is a `tracing_subscriber` [`Layer`] that records every span's fields
//! into shared storage. A test installs it as the default subscriber for its own scope,
//! drives real code through it, and inspects what actually got recorded - instead of
//! trusting that a span macro carries the fields it looks like it carries.
//!
//! Compiled for tests only (`#[cfg(test)]` on the `mod test_support;` line in
//! `main.rs`), so none of this ships.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tracing::Subscriber;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// One span this recorder saw, with every field recorded on it so far.
#[derive(Clone, Debug, Default)]
pub struct CapturedSpan {
    pub name: &'static str,
    pub fields: BTreeMap<String, String>,
}

#[derive(Default)]
struct Recorded {
    // Keyed by the tracing span id (as `u64`) so a later `on_record` call - a span field
    // declared `tracing::field::Empty` and filled in after the span opens - updates the
    // right entry instead of appending a duplicate.
    spans: Vec<(u64, CapturedSpan)>,
}

/// A `tracing_subscriber::Layer` that records what it sees. Clone freely - every clone
/// shares the same underlying storage, so the test keeps one handle and the layer keeps
/// another.
#[derive(Clone, Default)]
pub struct Recorder(Arc<Mutex<Recorded>>);

impl Recorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every span recorded so far, in the order it was opened.
    pub fn spans(&self) -> Vec<CapturedSpan> {
        self.0
            .lock()
            .expect("recorder lock")
            .spans
            .iter()
            .map(|(_, span)| span.clone())
            .collect()
    }
}

/// Collects a span's fields into a plain string map.
///
/// `Visit`'s default methods for every typed `record_*` (str, bool, i64, u64, ...) all
/// forward to `record_debug`, so overriding only that one method here is enough to catch
/// every field type a call site might record.
struct FieldCollector<'a>(&'a mut BTreeMap<String, String>);

impl Visit for FieldCollector<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

impl<S> Layer<S> for Recorder
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        attrs.record(&mut FieldCollector(&mut fields));
        let span = CapturedSpan {
            name: attrs.metadata().name(),
            fields,
        };
        self.0
            .lock()
            .expect("recorder lock")
            .spans
            .push((id.into_u64(), span));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, _ctx: Context<'_, S>) {
        let mut recorded = self.0.lock().expect("recorder lock");
        if let Some((_, span)) = recorded
            .spans
            .iter_mut()
            .find(|(seen, _)| *seen == id.into_u64())
        {
            values.record(&mut FieldCollector(&mut span.fields));
        }
    }
}
