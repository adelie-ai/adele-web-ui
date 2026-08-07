// TODO(adele-web-ui#91, red commit): remove once request_span is implemented.
#![allow(unused_imports, dead_code)]

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
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

/// One `http.request` span per inbound request, plus the request-count and duration
/// metrics.
///
/// Layer this as the OUTERMOST middleware (see `main.rs`) so `status` reflects what the
/// browser actually received, after every other layer - auth, the static-asset fallback -
/// has run.
pub async fn request_span(_req: Request, _next: Next) -> Response {
    todo!("create the http.request span, run `next` inside it, and record status + metrics")
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
}
