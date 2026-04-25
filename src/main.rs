use std::{net::SocketAddr};
use hyper::{Server, service::service_fn};
use tower::make::Shared;
use route_iq::middleware::log;

// start app
#[tokio::main]
async fn main() {

    // log wrapping our handle
    let make_service = Shared::new(service_fn(log));

    // proxy address & hyper server
    let addr = SocketAddr::from(([127,0,0,1], 3000));
    let server = Server::bind(&addr).serve(make_service);

    // run server
    if let Err(e) = server.await {
        println!("error: {}", e);
    }
}
