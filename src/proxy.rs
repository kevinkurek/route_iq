
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

use hyper::{Body, 
            Client, 
            Request, 
            Response};

// handle requests
pub async fn handle(req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    // Ok(Response::new(Body::from("Hello from HTTP proxy")))

    // hyper::Client sends request to backend 8080 server
    let client = Client::new();
    client.request(req).await
}