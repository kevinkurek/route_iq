use std::sync::Arc;

use hyper::{Body, 
            Request, 
            Response};
use crate::{load_balancing::LoadBalancingStrategy, proxy::{AppState, handle}};          


// logging
pub async fn log<B: LoadBalancingStrategy>(
    req: Request<Body>,
    state: Arc<AppState<B>>
) -> Result<Response<Body>, hyper::Error>{
    let path = req.uri().path();

    if path.starts_with("/api") {
        println!("API Path: {}", path)
    } else {
        println!("Generic Path: {}", path);
    }

    handle(req, state).await
}