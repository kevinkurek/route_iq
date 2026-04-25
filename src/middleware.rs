use hyper::{Body, 
            Request, 
            Response};
use crate::proxy::handle;          


// logging
pub async fn log(req: Request<Body>) -> Result<Response<Body>, hyper::Error>{
    let path = req.uri().path();

    if path.starts_with("/api") {
        println!("API Path: {}", path)
    } else {
        println!("Generic Path: {}", path);
    }

    handle(req).await
}