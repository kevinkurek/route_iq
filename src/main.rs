use std::time::{Duration, Instant};
use std::{net::SocketAddr, sync::Arc};
use std::sync::atomic::AtomicU64;
use hyper::{Server, service::service_fn};
use route_iq::load_balancing::{LeastConnections, LoadBalancingStrategy, refresh_health};
use tower::make::Shared;
use route_iq::{
    load_balancing::{Backend, RoundRobin},
    proxy::{AppState, handle},
};
use tracing::{debug, info};

// Decision-engine tuning. Short values during testing so oha bursts can drive
// a visible swap within seconds. Bump back up for production-realistic behavior
// (e.g. DECISION_TICK = 5s, SWAP_COOLDOWN = 60s).
const DECISION_TICK: Duration = Duration::from_secs(2);
const SWAP_COOLDOWN: Duration = Duration::from_secs(10);

// Which percentile to use for the trigger. With backend.rs injecting slow
// responses 5% of the time, p95 sits at the boundary (~15ms) and never crosses
// the threshold — p99 is the right signal at this injection rate.
const TRIGGER_PCT:     f64 = 0.99;
const TRIGGER_LATENCY: Duration = Duration::from_secs(1);
const CALM_LATENCY:    Duration = Duration::from_millis(200);

// start app
#[tokio::main]
async fn main() {

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
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

    let state = Arc::new(AppState::new(Box::new(RoundRobin::new()), backends));

    // probe health checks every 2 seconds
    let probe_state = Arc::clone(&state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let mut backends = probe_state.backends.lock().await;
            info!("probing backends");
            refresh_health(&probe_state.checker, &mut backends).await;
        }
    });

    // check p95 latency and if we need to switch load balancing strategies
    let decide_state = Arc::clone(&state);
    tokio::spawn(async move {
        // start with cooldown elapsed so the first eligible tick can act
        let mut last_swap = Instant::now() - SWAP_COOLDOWN;
        loop {
            tokio::time::sleep(DECISION_TICK).await;

            // Find the slowest backend's trigger-percentile latency.
            let worst = {
                let mut max: Option<(String, Duration)> = None;
                for (id, m) in decide_state.metrics.iter() {
                    if let Ok(guard) = m.try_lock() {
                        if let Some(p) = guard.percentile(TRIGGER_PCT) {
                            if max.as_ref().map_or(true, |(_, mp)| p > *mp) {
                                max = Some((id.clone(), p));
                            }
                        }
                    }
                }
                max
            };

            // Check all-calm condition for swap-back.
            let all_calm = decide_state.metrics.values().all(|m| {
                m.try_lock()
                    .ok()
                    .and_then(|g| g.percentile(TRIGGER_PCT))
                    .map(|p| p < CALM_LATENCY)
                    .unwrap_or(true)
            });

            let current = decide_state.balancer.read().await.name();

            // Per-tick visibility — turn on with RUST_LOG=route_iq=debug.
            debug!(
                current,
                worst_backend = ?worst.as_ref().map(|(id, _)| id),
                worst_latency_ms = ?worst.as_ref().map(|(_, p)| p.as_millis()),
                all_calm,
                pct = TRIGGER_PCT,
                "decision tick"
            );

            let target: Option<Box<dyn LoadBalancingStrategy>> = match (current, &worst) {
                ("round_robin", Some((_, p))) if *p > TRIGGER_LATENCY => {
                    Some(Box::new(LeastConnections::new()))
                }
                ("least_connections", _) if all_calm => {
                    Some(Box::new(RoundRobin::new()))
                }
                _ => None,
            };

            if let Some(new_strategy) = target {
                if last_swap.elapsed() >= SWAP_COOLDOWN {
                    let new_name = new_strategy.name();
                    info!(from = current, to = new_name, "decision engine: swap");
                    *decide_state.balancer.write().await = new_strategy;
                    last_swap = Instant::now();
                } else {
                    info!(
                        from = current,
                        cooldown_remaining = ?(SWAP_COOLDOWN - last_swap.elapsed()),
                        "swap suppressed by cooldown"
                    );
                }
            }
        }
    });

    // service factory — calls proxy::handle directly for each request
    let make_service = Shared::new(service_fn({
        let state = Arc::clone(&state);
        move |req| handle(req, Arc::clone(&state))
    }));

    // proxy address & hyper server
    let addr = SocketAddr::from(([127,0,0,1], 3000));
    let server = Server::bind(&addr).serve(make_service);

    // run server
    if let Err(e) = server.await {
        println!("error: {}", e);
    }
}
