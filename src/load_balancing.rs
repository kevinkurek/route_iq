use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
use tracing::{debug, info, warn};
use hyper::Client;

// define
// Backend - servers we'll connect to
// LoadBalanceStrategy - trait we'll use to create RoundRobin & LeastConnetions
// RoundRobin - struct which will define next backend to connect to, sequentially
// LeastConnections - struct which will define next backend to connect to with least connections

// Backend Server Structure
pub struct Backend {
    pub addr: String,
    pub id: String,
    pub active_connections: AtomicU64,
    pub healthy: bool,
}

// Generalized trait for algo routing
pub trait LoadBalancingStrategy: Send + Sync {
    fn pick_backend(&self, backends: &[Backend]) -> Option<usize>;
    fn name(&self) -> &'static str;
}

// async trait for health checks
#[async_trait::async_trait]
pub trait HealthCheck: Send + Sync {
    async fn is_healthy(&self, backend: &Backend) -> bool;
}

pub struct RoundRobin {
    // just grab next server in round-robin fashion
    next: AtomicUsize,
}

// state of servers lives in AppState defined in proxy.rs
// so LeastConnections shouldn't maintain server state
pub struct LeastConnections;

pub struct HttpHealthCheck;

impl RoundRobin {
    pub fn new() -> Self {
        Self {
            next: AtomicUsize::new(0),
        }
    }
}

impl LeastConnections {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl HealthCheck for HttpHealthCheck {
    async fn is_healthy(&self, backend: &Backend) -> bool {

        // perform health check with recurring N second probe to avoid blocking per request
        let url = format!("{}/health", backend.addr).parse::<hyper::Uri>().unwrap();
        let client = Client::new();
        match client.get(url).await {
            Ok(resp) if resp.status().is_success() => {
                debug!(backend = %backend.id, status = %resp.status(), "probe ok");
                true
            },
            Ok(resp) => {
                warn!(backend = %backend.id, status = %resp.status(), "probe unhealthy");
                false
            }
            Err(e) => {
                warn!(backend = %backend.id, error = %e, "probe failed");
                false
            }
        }
    }
}

// dependency injection checker into a health-refresh function
pub async fn refresh_health(
    checker: &impl HealthCheck,
    backends: &mut [Backend],
) {
    for backend in backends.iter_mut() {
        backend.healthy = checker.is_healthy(backend).await;
    }

    info!(
        healthy = ?backends.iter().filter(|b| b.healthy).map(|b| &b.id).collect::<Vec<_>>(),
        unhealthy = ?backends.iter().filter(|b| !b.healthy).map(|b| &b.id).collect::<Vec<_>>(),
        "probe cycle complete"
    )
}

fn log_unhealthy_skips(strategy: &str, backends: &[Backend]) {
    if backends.iter().any(|b| !b.healthy) {
        warn!(
            strategy = strategy,
            skipped = ?backends.iter().filter(|b| !b.healthy).map(|b| &b.id).collect::<Vec<_>>(),
            "skipping unhealthy"
        )
    }
}

impl LoadBalancingStrategy for RoundRobin {
    fn pick_backend(&self, backends: &[Backend]) -> Option<usize> {
        // if backends are healthy, then return [Some(i), ... Some(i)] connections
        let healthy: Vec<usize> = backends
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.healthy.then_some(i))
            .collect();

        // trace unhealthy skips
        log_unhealthy_skips("rr", backends);

        // check if at least one is healthy
        if healthy.is_empty() {
            return None;
        }

        // reads the current counter value and increments it by 1 atomically, returns the current value
        let n = self.next.fetch_add(1, Ordering::Relaxed);

        // Pick the next healthy backend in round-robin order:
        // - n is an incrementing counter (0, 1, 2, 3, ...)
        // - healthy.len() is how many healthy backends we currently have
        // - n % healthy.len() wraps the counter so it stays in bounds
        // Example: if healthy = [0, 2, 5], picks go 0 -> 2 -> 5 -> 0 -> 2 -> 5 ...
        Some(healthy[n % healthy.len()])
    }

    fn name(&self) -> &'static str {
        "round_robin"
    }
}

impl LoadBalancingStrategy for LeastConnections {
    fn pick_backend(&self, backends: &[Backend]) -> Option<usize> {

        // trace skipped backend if unhealthy
        log_unhealthy_skips("lc", backends);

        // actually select port with least connections
        backends
            .iter()
            .enumerate()
            .filter(|(_, b)| b.healthy)
            .min_by_key(|(i,b)| (b.active_connections.load(Ordering::Relaxed), *i))
            .map(|(i,_)| i)
    }

    fn name(&self) -> &'static str {
        "least_connections"
    }
}

// create testing
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_robin_name(){
        let rr = RoundRobin::new();
        let name = rr.name();
        assert_eq!(name, "round_robin");
    }

    #[test]
    fn round_robin_picks_healthy_backends_only() {
        let rr = RoundRobin::new();
        let backends = vec![
            Backend {addr: "http://127.0.0.1:8080".to_owned(), id: "a".into(), active_connections: AtomicU64::new(2), healthy: true},
            Backend {addr: "http://127.0.0.1:8081".to_owned(), id: "b".into(), active_connections: AtomicU64::new(3), healthy: false},
            Backend {addr: "http://127.0.0.1:8082".to_owned(), id: "c".into(), active_connections: AtomicU64::new(10), healthy: true},
            Backend {addr: "http://127.0.0.1:8083".to_owned(), id: "d".into(), active_connections: AtomicU64::new(10), healthy: true},
        ];

        let mut picks = vec![];
        for _ in 0..5 {
            let idx = rr.pick_backend(&backends).expect("should pick a backend");
            println!("picked index={idx}, id={}", backends[idx].id);
            picks.push(&backends[idx].id);
            assert!(backends[idx].healthy);
        }
        assert_eq!(vec!["a", "c", "d", "a", "c"], picks);
    }

    #[tokio::test]
    async fn check_health_statuses() {
        let mut backends = vec![
            Backend {addr: "http://127.0.0.1:8080".to_owned(), id: "a".into(), active_connections: AtomicU64::new(2), healthy: true},
            Backend {addr: "http://127.0.0.1:8081".to_owned(), id: "b".into(), active_connections: AtomicU64::new(3), healthy: false},
            Backend {addr: "http://127.0.0.1:8082".to_owned(), id: "c".into(), active_connections: AtomicU64::new(10), healthy: true},
            Backend {addr: "http://127.0.0.1:8083".to_owned(), id: "d".into(), active_connections: AtomicU64::new(10), healthy: true},
        ];

        let checker = HttpHealthCheck;
        refresh_health(&checker, &mut backends).await;
    }

    #[test]
    fn least_connections_picks_server_with_smallest_connections() {
        let lc = LeastConnections::new();
        let backends = vec![
            Backend {addr: "http://127.0.0.1:8080".to_owned(), id: "a".into(), active_connections: AtomicU64::new(2), healthy: true},
            Backend {addr: "http://127.0.0.1:8081".to_owned(), id: "b".into(), active_connections: AtomicU64::new(3), healthy: false},
            Backend {addr: "http://127.0.0.1:8082".to_owned(), id: "c".into(), active_connections: AtomicU64::new(10), healthy: true},
            Backend {addr: "http://127.0.0.1:8083".to_owned(), id: "d".into(), active_connections: AtomicU64::new(10), healthy: true},
        ];

        let idx = lc.pick_backend(&backends).expect("should pick a backend");
        println!("Least Connections Backend Selected: {}", backends[idx].id);
        assert_eq!(backends[idx].id, "a".to_string());
    }
}