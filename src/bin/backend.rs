use std::convert::Infallible;
use std::env;
use std::time::Duration;

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};

async fn health(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    Ok(Response::new(Body::from("healthy")))
}

async fn work(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    tokio::time::sleep(Duration::from_millis(10)).await;
    Ok(Response::new(Body::from("work done.")))
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
