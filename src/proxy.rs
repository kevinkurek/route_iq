// Reverse-proxy core. `AppState` holds the backend list + selection strategy +
// shared outbound HTTP client. `handle` is the per-request entry point: it
// picks a backend, rewrites the URI, forwards via hyper::Client, and tracks
// per-backend in-flight connection counts on the way in / out.
//
// See README "Request lifecycle" for the end-to-end diagram.

use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::info;
use std::sync::atomic::Ordering;

use hyper::{Body, Client, Method, Request, Response, StatusCode};

use crate::load_balancing::{Backend, HttpHealthCheck, LeastConnections, LoadBalancingStrategy, RoundRobin};

pub struct AppState {
    pub backends: Mutex<Vec<Backend>>,
    pub balancer: RwLock<Box<dyn LoadBalancingStrategy>>, // required for POST /admin/strategy
    pub checker: HttpHealthCheck,
    pub client: Client<hyper::client::HttpConnector>
}

impl AppState {
    pub fn new(balancer: Box<dyn LoadBalancingStrategy>, backends: Vec<Backend>) -> Self {
        Self {
            backends: Mutex::new(backends),
            balancer: RwLock::new(balancer),
            checker: HttpHealthCheck,
            client: Client::new(),
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
    result
}