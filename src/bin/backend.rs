use std::convert::Infallible;
use std::env;

use hyper::service::{make_service_fn, service_fn};
use hyper::Server;

use route_iq::backend::handle;

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
