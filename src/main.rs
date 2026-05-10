use std::time::Duration;
use std::{net::SocketAddr, sync::Arc};
use std::sync::atomic::AtomicU64;
use hyper::{Server, service::service_fn};
use route_iq::load_balancing::refresh_health;
use tower::make::Shared;
use route_iq::{
    load_balancing::{Backend, RoundRobin},
    middleware::log,
    proxy::AppState,
};

// start app
#[tokio::main]
async fn main() {

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .compact()
        .init();

    // Production backend topology. Each entry must have a `backend` process
    // listening on the matching port (see README "Run The Stack").
    let backends = vec![
        Backend { addr: "http://127.0.0.1:8080".into(), id: "a".into(), active_connections: AtomicU64::new(0), healthy: true },
        Backend { addr: "http://127.0.0.1:8081".into(), id: "b".into(), active_connections: AtomicU64::new(0), healthy: true },
        Backend { addr: "http://127.0.0.1:8082".into(), id: "c".into(), active_connections: AtomicU64::new(0), healthy: true },
        Backend { addr: "http://127.0.0.1:8083".into(), id: "d".into(), active_connections: AtomicU64::new(0), healthy: true },
    ];

    let state = Arc::new(AppState::new(RoundRobin::new(), backends));

    // probe health checks every 2 seconds
    let probe_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let mut backends = probe_state.backends.lock().await;
            println!("probing backend for health check");
            refresh_health(&probe_state.checker, &mut backends).await;
        }
    });

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
