use std::sync::atomic::{AtomicUsize, Ordering};

// define
// Backend - servers we'll connect to
// LoadBalanceStrategy - trait we'll use to create RoundRobin & LeastConnetions
// RoundRobin - struct which will define next backend to connect to, sequentially
// LeastConnections - struct which will define next backend to connect to with least connections

// Backend Server Structure
pub struct Backend {
    pub id: String,
    pub active_connections: usize,
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
    // just grab next server in round-robin
    next: AtomicUsize,
}

pub struct LeastConnections {
    next: AtomicUsize,
}

pub struct HttpHealthCheck;

impl RoundRobin {
    pub fn new() -> Self {
        Self {
            next: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl HealthCheck for HttpHealthCheck {
    async fn is_healthy(&self, backend: &Backend) -> bool {
        // do async HTTP probe here; placeholder logic for now
        backend.healthy
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
}

impl LoadBalancingStrategy for RoundRobin {
    fn pick_backend(&self, backends: &[Backend]) -> Option<usize> {
        // if backends are healthy, then return [Some(i), ... Some(i)] connections
        let healthy: Vec<usize> = backends
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.healthy.then_some(i))
            .collect();

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
            Backend {id: "a".into(), active_connections: 2, healthy: true},
            Backend {id: "b".into(), active_connections: 3, healthy: false},
            Backend {id: "c".into(), active_connections: 10, healthy: true},
            Backend {id: "d".into(), active_connections: 10, healthy: true},
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
            Backend {id: "a".into(), active_connections: 2, healthy: true},
            Backend {id: "b".into(), active_connections: 3, healthy: false},
            Backend {id: "c".into(), active_connections: 10, healthy: true},
            Backend {id: "d".into(), active_connections: 10, healthy: true},
        ];

        let checker = HttpHealthCheck;
        refresh_health(&checker, &mut backends).await;
    }
}