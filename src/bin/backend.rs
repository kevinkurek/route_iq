use std::convert::Infallible;
use std::env;
use std::time::Duration;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use rand::RngExt;

async fn health(_req: Request<Body>) -> Result<Response<Body>, Infallible> {

    // simulate unhealthy every N% of the time
    let mut rng = rand::rng();
    let (status, body) = match rng.random_range(0..100) {
        0..5 => (StatusCode::INTERNAL_SERVER_ERROR, "unhealthy"),
        _ => (StatusCode::OK, "healthy")
    };

    // build resp; return status and body
    let resp = Response::builder()
        .status(status)
        .body(Body::from(body))
        .unwrap();
    Ok(resp)
}

async fn work(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    
    // 5% of time delay, 95% normal execution to test artifical delay
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

async fn handle(req: Request<Body>) -> Result<Response<Body>, Infallible> {
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {

    // Port is set via command line input
    // PORT=8080 ./target/debug/backend &
    // PORT=8081 ./target/debug/backend &
    // PORT=8082 ./target/debug/backend &
    // PORT=8083 ./target/debug/backend &
    let port = env::var("PORT")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "8080".to_string());
    let addr = format!("127.0.0.1:{port}").parse()?;

    let make_service = make_service_fn(|_conn| async {
        Ok::<_, Infallible>(service_fn(handle))
    });

    println!("HTTP server listening on http://{addr}");
    Server::bind(&addr).serve(make_service).await?;

    Ok(())
}
