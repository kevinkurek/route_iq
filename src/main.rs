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

use std::{convert::Infallible, net::SocketAddr};
use tower::make::Shared;

use hyper::{Body, 
            Client, 
            Request, 
            Response, 
            Server, 
            client::service, 
            service::{make_service_fn, service_fn}};

// logging
// async fn log(req: Request<Body>) -> Result<Response<Body>, hyper::Error>{
//     Ok()
// }

// handle requests
async fn handle(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    Ok(Response::new(Body::from("Hello from HTTP proxy")))
}

// start app
#[tokio::main]
async fn main() {

    // factory that takes a request and returns a response
    // let make_service = Shared::new(service_fn(log));
    let make_service = make_service_fn(|_| async {
        Ok::<_, Infallible>(service_fn(handle))
    });

    // proxy address & hyper server
    let addr = SocketAddr::from(([127,0,0,1], 3000));
    let server = Server::bind(&addr).serve(make_service);

    // run server
    if let Err(e) = server.await {
        println!("error: {}", e);
    }
}
