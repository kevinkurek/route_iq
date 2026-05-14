// Reverse-proxy core. `AppState` holds the backend list + selection strategy +
// shared outbound HTTP client. `handle` is the per-request entry point: it
// picks a backend, rewrites the URI, forwards via hyper::Client, and tracks
// per-backend in-flight connection counts on the way in / out.
//
// See README "Request lifecycle" for the end-to-end diagram.

use std::{collections::{HashMap, VecDeque}, sync::Arc, time::{Duration, Instant}};
use tokio::sync::{Mutex, RwLock};
use tracing::info;
use std::sync::atomic::Ordering;

use hyper::{Body, Client, Method, Request, Response, StatusCode};

use crate::load_balancing::{Backend, HttpHealthCheck, LeastConnections, LoadBalancingStrategy, RoundRobin};

pub struct Sample {
    at: Instant,
    latency: Duration,
}

pub struct Metrics {
    samples: VecDeque<Sample>,
    window: Duration,
}

impl Metrics {
    pub fn new() -> Self {
        // Window tuned for interactive testing — short bursts of oha load.
        // Bump up (e.g. 30s) for production-realistic smoothing.
        Self {
            samples: VecDeque::new(),
            window: Duration::from_secs(10),
        }
    }

    pub fn record(&mut self, latency: Duration) {
        let now = Instant::now();
        self.samples.push_back(Sample { at: now, latency });

        // Drop anything older than the window. Cheap — front-only pops until
        // we hit a sample still inside the window.
        let cutoff = now - self.window;
        while let Some(front) = self.samples.front() {
            if front.at < cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn percentile(&self, p: f64) -> Option<Duration> {
        if self.samples.is_empty() {return None;}
        let mut latencies: Vec<Duration> = self.samples.iter().map(|s| s.latency).collect();
        latencies.sort();
        let idx = ((latencies.len() as f64) * p).floor() as usize;
        latencies.get(idx).copied()
    }
}

pub struct AppState {
    pub backends: Mutex<Vec<Backend>>,
    pub balancer: RwLock<Box<dyn LoadBalancingStrategy>>, // required for POST /admin/strategy
    pub checker: HttpHealthCheck,
    pub client: Client<hyper::client::HttpConnector>,
    pub metrics: HashMap<String, Mutex<Metrics>>, // HashMap<backendId, Metrics> for p95 latency checks
}

impl AppState {
    pub fn new(
        balancer: Box<dyn LoadBalancingStrategy>,
        backends: Vec<Backend>,
    ) -> Self {
        // One Metrics entry per backend, keyed by id.
        let metrics: HashMap<String, Mutex<Metrics>> = backends
            .iter()
            .map(|b| (b.id.clone(), Mutex::new(Metrics::new())))
            .collect();

        Self {
            backends: Mutex::new(backends),
            balancer: RwLock::new(balancer),
            checker: HttpHealthCheck,
            client: Client::new(),
            metrics,
        }
    }
}

// change load balancing strategy using a POST to /admin/strategy
async fn admin_set_strategy(
    req: Request<Body>, 
    state: Arc<AppState>
) -> Result<Response<Body>, hyper::Error> {
    // extract strategy name from path
    let name = req.uri().path().trim_start_matches("/admin/strategy/");

    let new_strategy: Box<dyn LoadBalancingStrategy> = match name {
        "round_robin" => Box::new(RoundRobin::new()),
        "least_connections" => Box::new(LeastConnections::new()),
        _ => {
            let mut resp = Response::new(Body::from(format!("unknown strategy: {name}")));
            *resp.status_mut() = StatusCode::BAD_REQUEST;
            return Ok(resp);
        }
    };

    let mut current_strategy = state.balancer.write().await;
    info!(strategy = name, "strategy swithced");
    *current_strategy = new_strategy;
    
    Ok(Response::new(Body::from(format!("switched to {name}\n"))))
}

// handle requests w/ load balancing strategy selecting the backend
#[tracing::instrument(
    skip(req, state),
    fields(method = %req.method(), path = %req.uri().path())
)]
pub async fn handle(
    req: Request<Body>,
    state: Arc<AppState>,
) -> Result<Response<Body>, hyper::Error> {

    // determine if it's a load balancing strategy switch first
    if req.method() == Method::POST && req.uri().path().starts_with("/admin/strategy/"){
        return admin_set_strategy(req, state).await;
    }

    let request_start = Instant::now(); // start the timer

    let (selected_backend_addr, selected_backend_id) = {

        // locks mutex to ensure backend selection is static during LoadBalancingStrategy backend selection
        let backends = state.backends.lock().await;

        // balancer now goes through RwLock due to POST /admin/strategy requiring trait object
        let balancer = state.balancer.read().await;

        match balancer.pick_backend(&backends) {
            Some(i) => {

                // increment the connection before you send request
                backends[i].active_connections.fetch_add(1, Ordering::Relaxed);
                (backends[i].addr.clone(), backends[i].id.clone())
            }
            None => {
                let mut resp = Response::new(Body::from("No healthy backend"));
                *resp.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
                return Ok(resp)
            }
        } 
    };
    
    info!(backend = %selected_backend_addr, "selected");

    let path_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    // update the req with the selected backend address & path query
    let uri: hyper::Uri = format!("{}{}", selected_backend_addr, path_query)
        .parse()
        .expect("valid backend URI");

    // shadow then make req mutable then send request
    let mut req = req;
    *req.uri_mut() = uri;

    let result = state.client.request(req).await;

    {
        // now we decrement the active connection because we sent the request and result
        // has now returned, thus that connection is free
        let mut backends = state.backends.lock().await;
        if let Some(b) = backends.iter_mut().find(|b| b.id == selected_backend_id) {
            b.active_connections.fetch_sub(1, Ordering::Relaxed);
        }
    }

    // record latency for the backend we used
    if let Some(metrics) = state.metrics.get(&selected_backend_id) {
        metrics.lock().await.record(request_start.elapsed());
    }

    result
}


// create testing
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_of_known_samples() {
        let mut m = Metrics::new();
        for ms in [10, 20, 30, 40, 50, 60, 70, 80, 90, 100] {
            m.record(Duration::from_millis(ms));
        }

        // 10 samples, p95 → index floor(10 * 0.95) = 9 → 100ms
        assert_eq!(m.percentile(0.95), Some(Duration::from_millis(100)));
        // p50 → index 5 → 60ms
        assert_eq!(m.percentile(0.50), Some(Duration::from_millis(60)));
        // empty case
        let empty = Metrics::new();
        assert_eq!(empty.percentile(0.95), None);
    }
}