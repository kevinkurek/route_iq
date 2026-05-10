
// External Client (browser, curl)
//         |
//         v
// [ Your App ]
//   (hyper::Server)
//         |
//         v
//    your logic
//         |
//         v
//   (hyper::Client)
//         |
//         v
// Backend Server

// make_service_fn
// It is a factory: for each new connection accepted by Hyper Server, it creates a service instance.
// I.E. 1000 connections come in at once, 1000 service instances "service_fn(handle)" are spun up 
// It is per connection, not per request.
// A single connection can carry multiple requests, and they go through that connection’s one service instance.
// For HTTP/2, many requests can be concurrent on one connection, still using that connection’s service.

// service_fn(handle)
// It adapts your async function handle into Hyper’s Service trait, so Hyper can call it for each HTTP request.

// factory that takes a request and returns a response
// let make_service = make_service_fn(|_| async {
//     Ok::<_, Infallible>(service_fn(handle))
// });

// allows us to create one shared handler for all incoming connections
// allows us to do things like reuse one client for outbound backend calls
// only works if service function is clonable
// let make_service = Shared::new(service_fn(handle));

use std::sync::Arc;
use tokio::sync::Mutex;
use std::sync::atomic::Ordering;

use hyper::{Body, 
            Client, 
            Request, 
            Response, StatusCode};

use crate::load_balancing::{Backend, HttpHealthCheck, LoadBalancingStrategy};

pub struct AppState<B: LoadBalancingStrategy> {
    pub backends: Mutex<Vec<Backend>>,
    pub balancer: B,
    pub checker: HttpHealthCheck,
    pub client: Client<hyper::client::HttpConnector>
}

impl<B: LoadBalancingStrategy> AppState<B> {
    pub fn new(balancer: B, backends: Vec<Backend>) -> Self {
        Self {
            backends: Mutex::new(backends),
            balancer,
            checker: HttpHealthCheck,
            client: Client::new(),
        }
    }
}

// handle requests
pub async fn handle<B: LoadBalancingStrategy>(
    req: Request<Body>,
    state: Arc<AppState<B>>,
) -> Result<Response<Body>, hyper::Error> {
    // Ok(Response::new(Body::from("Hello from HTTP proxy")))

    let (selected_backend_addr, selected_backend_id) = {
        let backends = state.backends.lock().await;

        match state.balancer.pick_backend(&backends) {
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
    
    println!("Selected backend: {}", selected_backend_addr);

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