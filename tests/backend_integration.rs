// Integration tests for the backend HTTP handlers.
//
// These call `route_iq::backend::handle` directly with constructed Request
// values. No subprocess, no network sockets, no port conflicts — just unit-y
// integration tests of the routing + handler shape.
//
// Note: backend handlers (/health, /work) use random latency and random
// status codes by design (so RR vs LC actually diverge under load).
// We deliberately scope assertions to the *deterministic* parts:
//   - the dispatcher correctly maps method+path to a handler
//   - the response shape is well-formed
//   - /work introduces *some* delay (latency floor)

use std::time::{Duration, Instant};

use hyper::{Body, Method, Request, StatusCode};
use route_iq::backend::handle;

#[tokio::test]
async fn unknown_path_returns_404() {
    // The only fully deterministic route — the `_` arm of the dispatcher.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/this-route-does-not-exist")
        .body(Body::empty())
        .unwrap();

    let resp = handle(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unknown_method_on_known_path_returns_404() {
    // Dispatcher matches on (method, path) — POST /health should miss the
    // GET /health arm and fall through to the not-found branch.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let resp = handle(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn health_route_returns_either_200_or_500() {
    // /health randomly returns 500 ~5% of the time, so we can't pin the exact
    // status. What we *can* assert: the route matched (we got a real response,
    // not a 404), and the status is one of the documented outcomes.
    let req = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())
        .unwrap();

    let resp = handle(req).await.unwrap();
    let status = resp.status();
    assert!(
        status == StatusCode::OK || status == StatusCode::INTERNAL_SERVER_ERROR,
        "unexpected status from /health: {status}"
    );
}

#[tokio::test]
async fn work_route_introduces_minimum_latency() {
    // /work always sleeps at least 8ms before responding. That floor is
    // deterministic even though the upper bound is random — so we assert
    // the call took at least 8ms, which proves the handler actually
    // awaited the sleep (the classic missing-`.await` bug would fail this).
    let req = Request::builder()
        .method(Method::GET)
        .uri("/work")
        .body(Body::empty())
        .unwrap();

    let start = Instant::now();
    let resp = handle(req).await.unwrap();
    let elapsed = start.elapsed();

    let status = resp.status();
    let acceptable = [
        StatusCode::OK,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::GATEWAY_TIMEOUT,
    ];
    assert!(
        acceptable.contains(&status),
        "unexpected status from /work: {status}"
    );
    assert!(
        elapsed >= Duration::from_millis(8),
        "/work should sleep at least 8ms, only took {:?}",
        elapsed
    );
}
