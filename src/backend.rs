// Backend HTTP handlers.
//
// Lives in the library (not just the binary) so integration tests can call
// `handle` directly with constructed Request<Body> values instead of spawning
// the binary as a subprocess. The `bin/backend.rs` wrapper is just a thin
// hyper::Server harness around `handle`.

use std::convert::Infallible;
use std::time::Duration;

use hyper::{Body, Method, Request, Response, StatusCode};
use rand::RngExt;

pub async fn health(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    // simulate unhealthy every N% of the time
    let mut rng = rand::rng();
    let (status, body) = match rng.random_range(0..100) {
        0..5 => (StatusCode::INTERNAL_SERVER_ERROR, "unhealthy"),
        _ => (StatusCode::OK, "healthy"),
    };

    let resp = Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap();
    Ok(resp)
}

pub async fn work(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    // 5% of time delay, 95% normal execution to test artificial delay
    let (delay_ms, status, body) = {
        let mut rng = rand::rng();
        let delay_ms = if rng.random_bool(0.05) {
            // 200ms-2s work 5% of the time
            rng.random_range(200..2000)
        } else {
            // 8-15ms, normal work execution time
            rng.random_range(8..15)
        };

        // different error rates to simulate failing servers
        let (status, body) = match rng.random_range(0..100) {
            0..6 => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
            6..9 => (StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
            9 => (StatusCode::GATEWAY_TIMEOUT, "timeout"),
            _ => (StatusCode::OK, "work done."),
        };
        (delay_ms, status, body)
    }; // rng dropped here, before any await

    // rng !Send, so must drop before awaiting
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;

    let resp = Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap();
    Ok(resp)
}

pub async fn handle(req: Request<Body>) -> Result<Response<Body>, Infallible> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/work") => work(req).await,
        (&Method::GET, "/health") => health(req).await,
        _ => {
            let mut resp = Response::new(Body::from("not found"));
            *resp.status_mut() = StatusCode::NOT_FOUND;
            Ok(resp)
        }
    }
}
