//! Per-request tracing spans and metrics for the BFF's axum router.
//!
//! Every inbound HTTP request gets one span, `http.request`, carrying its method, path
//! and (once known) status. The span assumes nothing about its own parent - a
//! `traceparent` the browser sends becomes this span's parent once
//! `desktop-assistant#1152` lands, and nothing here has to change for that to work. The
//! BFF is the first hop that can export a turn's trace, not the root of it; see the
//! epic's "Where the root span goes, and why the BFF is a special case" note on
//! `adele-web-ui#91`.
//!
//! Content never appears here. A method, a path and a status code are not a prompt or a
//! reply body, so nothing on this path needs the D10 level contract lifted past INFO.

use std::time::Instant;

use adelie_telemetry::metrics::{self, Label};
use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

/// One `http.request` span per inbound request, plus the request-count and duration
/// metrics.
///
/// Layer this as the OUTERMOST middleware (see `main.rs`) so `status` reflects what the
/// browser actually received, after every other layer - auth, the static-asset fallback -
/// has run.
pub async fn request_span(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    // The metric label, not the span field: bounded to this binary's own small,
    // fixed set of registered routes plus "static". See `metric_route`'s own doc.
    let route = metric_route(&req);

    let span = tracing::info_span!(
        "http.request",
        method = %method,
        path = %path,
        status = tracing::field::Empty,
    );

    metrics::increment(
        "http.requests.started",
        &[Label::new("route", route.clone())],
    );

    let start = Instant::now();
    let response = async { next.run(req).await }.instrument(span.clone()).await;
    let elapsed = start.elapsed();

    let status = response.status().as_u16();
    span.record("status", status);

    metrics::record_duration(
        "http.request.duration",
        elapsed,
        &[
            Label::new("route", route.clone()),
            Label::new("status", status.to_string()),
        ],
    );
    metrics::increment("http.requests.completed", &[Label::new("route", route)]);

    response
}

/// The `route` metric label for this request: the matched route template (`/healthz`,
/// `/login`, ...) axum records once it has matched one of this binary's own registered
/// routes, or the fixed label `"static"` when nothing matched.
///
/// **Never the raw request path.** A caller chooses the path on every request; the SPA
/// does client-side routing and unknown paths fall through to `ServeFile(index.html)`,
/// so every conversation URL a person visits, every hashed asset filename, and every
/// path an unauthenticated scanner probes (`/.env` and the like) would otherwise be a
/// distinct label value. The registry's cardinality cap (64 per metric, first-come, no
/// eviction) is a backstop against unbounded memory growth, not a cardinality strategy:
/// once it is burned, every route - including the real ones - folds into
/// `cardinality=other` for the life of the process, and only a restart recovers it.
///
/// `MatchedPath` is populated by axum's own router before this middleware's `next.run`
/// returns control here (verified against axum 0.8.9: present for a matched route,
/// absent for a request the router's `fallback_service` served instead), so it is
/// controlled entirely by this binary's own `Router::route` calls, never by the caller.
/// A request with no matched route - the SPA's fallback, a static asset, a scanner -
/// carries no `MatchedPath` at all, and buckets into `"static"` alongside every other
/// one, which is what keeps this bounded at about five values permanently.
fn metric_route(req: &Request) -> String {
    req.extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| "static".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt;
    use tracing_subscriber::layer::SubscriberExt;

    use crate::test_support::Recorder;

    /// Acceptance: a span per inbound request, carrying method, path and status.
    #[tokio::test]
    async fn request_span_carries_method_path_and_status() {
        let recorder = Recorder::new();
        let subscriber = tracing_subscriber::registry().with(recorder.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        let app = Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn(request_span));

        let response = app
            .oneshot(
                HttpRequest::builder()
                    .method("GET")
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");
        assert_eq!(response.status(), StatusCode::OK);

        let spans = recorder.spans();
        let request_span = spans
            .iter()
            .find(|span| span.name == "http.request")
            .expect("an http.request span must be recorded for every inbound request");

        assert_eq!(
            request_span.fields.get("method").map(String::as_str),
            Some("GET"),
            "the span must carry the request method"
        );
        assert_eq!(
            request_span.fields.get("path").map(String::as_str),
            Some("/healthz"),
            "the span must carry the request path"
        );
        assert_eq!(
            request_span.fields.get("status").map(String::as_str),
            Some("200"),
            "the span must carry the response status, recorded once it is known"
        );
        assert_eq!(
            request_span.fields.len(),
            3,
            "no field beyond method, path and status may appear on the request span \
             (D10: paths and status are fine at INFO, bodies are not); saw {:?}",
            request_span.fields
        );
    }

    /// Every `route`-labelled value the "http.requests.completed" counter carries, as
    /// of this call. Reads the process-global registry - shared with every other test
    /// in this binary - so assertions built on it must check for the presence or
    /// absence of specific values, never an exact total, or they would be racy against
    /// whatever else runs concurrently.
    fn recorded_route_labels() -> std::collections::BTreeSet<String> {
        adelie_telemetry::metrics::global()
            .snapshot()
            .counters
            .into_iter()
            .filter(|counter| counter.name == "http.requests.completed")
            .flat_map(|counter| counter.labels.into_iter())
            .filter(|label| label.key() == "route")
            .map(|label| label.value().to_string())
            .collect()
    }

    /// Acceptance (review finding #1): a request that matches none of this binary's
    /// registered routes - the SPA's client-side routing, a static asset, a scanner
    /// probing arbitrary paths - must never turn its own raw path into a `route` metric
    /// label. Every one of these paths would otherwise be a distinct, permanent series
    /// against the registry's 64-value cardinality cap.
    #[tokio::test]
    async fn unmatched_paths_never_become_their_own_route_label() {
        let app = Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .fallback(|| async { StatusCode::NOT_FOUND })
            .layer(axum::middleware::from_fn(request_span));

        let probe_paths = [
            "/UNBOUNDED_ROUTE_LABEL_PROBE_1",
            "/UNBOUNDED_ROUTE_LABEL_PROBE_2/nested/segment",
            "/.env",
        ];
        for path in probe_paths {
            let _ = app
                .clone()
                .oneshot(
                    HttpRequest::builder()
                        .uri(path)
                        .body(Body::empty())
                        .expect("request builds"),
                )
                .await
                .expect("request completes");
        }

        let route_values = recorded_route_labels();
        for probe_path in probe_paths {
            assert!(
                !route_values.contains(probe_path),
                "an unmatched request's raw path {probe_path:?} must never become its \
                 own route label; every unmatched request must fold into \"static\" \
                 instead. Labels seen: {route_values:?}"
            );
        }
        assert!(
            route_values.contains("static"),
            "unmatched requests must be labelled \"static\". Labels seen: {route_values:?}"
        );
    }

    /// Acceptance (review finding #1): a request that DOES match one of this binary's
    /// registered routes keeps its own route label - the fix bounds the label space, it
    /// does not erase which real route was actually hit.
    #[tokio::test]
    async fn matched_route_keeps_its_own_route_label() {
        let app = Router::new()
            .route("/healthz", get(|| async { StatusCode::OK }))
            .layer(axum::middleware::from_fn(request_span));

        let _ = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .expect("request builds"),
            )
            .await
            .expect("request completes");

        assert!(
            recorded_route_labels().contains("/healthz"),
            "a matched route must keep its own route label"
        );
    }
}
