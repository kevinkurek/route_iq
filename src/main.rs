use std::{net::SocketAddr, sync::Arc};
use std::sync::atomic::AtomicU64;
use hyper::{Server, service::service_fn};
use tower::make::Shared;
use route_iq::{
    load_balancing::{Backend, RoundRobin},
    middleware::log,
    proxy::AppState,
};

// start app
#[tokio::main]
async fn main() {

    // Production backend topology. Each entry must have a `backend` process
    // listening on the matching port (see README "Run The Stack").
    let backends = vec![
        Backend { addr: "http://127.0.0.1:8080".into(), id: "a".into(), active_connections: AtomicU64::new(0), healthy: true },
        Backend { addr: "http://127.0.0.1:8081".into(), id: "b".into(), active_connections: AtomicU64::new(0), healthy: true },
        Backend { addr: "http://127.0.0.1:8082".into(), id: "c".into(), active_connections: AtomicU64::new(0), healthy: true },
        Backend { addr: "http://127.0.0.1:8083".into(), id: "d".into(), active_connections: AtomicU64::new(0), healthy: true },
    ];

    let state = Arc::new(AppState::new(RoundRobin::new(), backends));

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
