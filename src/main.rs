use std::{net::SocketAddr, sync::Arc};
use hyper::{Server, service::service_fn};
use tower::make::Shared;
use route_iq::{load_balancing::RoundRobin, middleware::log, proxy::AppState};

// start app
#[tokio::main]
async fn main() {

    let state = Arc::new(AppState::new(RoundRobin::new()));

    // log wrapping our handle
    let make_service = Shared::new(service_fn({
        let state = Arc::clone(&state);
        move |req| log(req, Arc::clone(&state))
    }));

    // proxy address & hyper server
    let addr = SocketAddr::from(([127,0,0,1], 3000));
    let server = Server::bind(&addr).serve(make_service);

    // run server
    if let Err(e) = server.await {
        println!("error: {}", e);
    }
}
