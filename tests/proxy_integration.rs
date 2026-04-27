// full integration test

use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use hyper::{Body, Request, Response, Server, service::{make_service_fn, service_fn}};
use route_iq::{
    load_balancing::{LeastConnections, RoundRobin},
    proxy::{AppState, handle},
};
use std::sync::atomic::{Ordering};


#[tokio::test]
async fn proxy_forwards_request_and_releases_connection() {

    // Start a tiny fake backend server that always returns "backend-ok".
    // This simulates the upstream service your proxy forwards traffic to.
    let backend_make = make_service_fn(|_| async {
        Ok::<_, Infallible>(service_fn(|_req| async {
            Ok::<_, Infallible>(Response::new(Body::from("backend-ok")))
        }))
    });

    // Bind that fake backend to an ephemeral local port (port 0),
    // then spawn it so it runs in the background during this test.
    let backend = Server::bind(&SocketAddr::from(([127,0,0,1], 0))).serve(backend_make);
    let backend_addr = backend.local_addr();
    tokio::spawn(backend);

    // Build proxy application state using RoundRobin load balancing.
    // Same type of state real app uses
    let state = Arc::new(AppState::new(RoundRobin::new()));

    // Create an HTTP request targeting the fake backend address.
    // The proxy handler will receive this request and forward it.
    let uri = format!("http://{}/hello", backend_addr);
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap();

    // Call the proxy handler directly and assert the HTTP status is ok
    let resp = handle(req, Arc::clone(&state)).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::OK);

    // Read the response body and verify it matches what backend returned
    // confirms request forwarding worked end-to-end
    let body = hyper::body::to_bytes(resp.into_body()).await.unwrap();
    assert_eq!(&body[..], b"backend-ok");

    // Confirm active connection counters were decremented back to zero
    // after request completion (no leaked "in-flight" connections).
    let backends = state.backends.lock().await;
    assert!(backends.iter().all(|b| b.active_connections.load(Ordering::Relaxed) == 0));
}

#[tokio::test]
async fn proxy_round_robin_splits_in_flight_connections_across_backends() {
    use tokio::time::{sleep, Duration};

    // Slow backend so requests stay in flight long enough to inspect counters.
    let backend_make = make_service_fn(|_| async {
        Ok::<_, Infallible>(service_fn(|_req| async {
            sleep(Duration::from_millis(200)).await;
            Ok::<_, Infallible>(Response::new(Body::from("slow-backend-ok")))
        }))
    });

    let backend = Server::bind(&SocketAddr::from(([127, 0, 0, 1], 0))).serve(backend_make);
    let backend_addr = backend.local_addr();
    tokio::spawn(backend);

    let state = Arc::new(AppState::new(RoundRobin::new()));

    // Build two requests so round robin gets called twice.
    let uri = format!("http://{}/rr", backend_addr);
    let req1 = Request::builder()
        .method("GET")
        .uri(&uri)
        .body(Body::empty())
        .unwrap();

    let req2 = Request::builder()
        .method("GET")
        .uri(&uri)
        .body(Body::empty())
        .unwrap();

    let s1 = Arc::clone(&state);
    let s2 = Arc::clone(&state);

    // Fire two concurrent proxy calls.
    let t1 = tokio::spawn(async move { handle(req1, s1).await });
    let t2 = tokio::spawn(async move { handle(req2, s2).await });

    // Give both requests time to be selected + incremented, but not completed.
    sleep(Duration::from_millis(50)).await;

    {
        let backends = state.backends.lock().await;
        assert_eq!(backends.len(), 4);
        assert_eq!(backends[0].active_connections.load(Ordering::Relaxed), 1);
        assert_eq!(backends[1].active_connections.load(Ordering::Relaxed), 1);
    }

    let r1 = t1.await.unwrap().unwrap();
    let r2 = t2.await.unwrap().unwrap();

    assert_eq!(r1.status(), hyper::StatusCode::OK);
    assert_eq!(r2.status(), hyper::StatusCode::OK);

    // After completion, counters should return to zero.
    let backends = state.backends.lock().await;
    assert!(backends.iter().all(|b| b.active_connections.load(Ordering::Relaxed) == 0));
}

#[tokio::test]
async fn proxy_least_connections_prefers_lowest_in_flight_backends() {
    use tokio::time::{sleep, Duration};

    // Slow backend keeps requests in-flight long enough to inspect counters.
    let backend_make = make_service_fn(|_| async {
        Ok::<_, Infallible>(service_fn(|_req| async {
            sleep(Duration::from_millis(200)).await;
            Ok::<_, Infallible>(Response::new(Body::from("slow-backend-ok")))
        }))
    });

    let backend = Server::bind(&SocketAddr::from(([127, 0, 0, 1], 0))).serve(backend_make);
    let backend_addr = backend.local_addr();
    tokio::spawn(backend);

    let state = Arc::new(AppState::new(LeastConnections::new()));

    // Seed baseline load so "a" is already busy and least-connections should prefer b/c first.
    {
        let backends = state.backends.lock().await;
        backends[0].active_connections.store(5, Ordering::Relaxed); // a
        backends[1].active_connections.store(0, Ordering::Relaxed); // b
        backends[2].active_connections.store(0, Ordering::Relaxed); // c
        backends[3].active_connections.store(0, Ordering::Relaxed); // d
    }

    let uri = format!("http://{}/lc", backend_addr);
    let req1 = Request::builder()
        .method("GET")
        .uri(&uri)
        .body(Body::empty())
        .unwrap();

    let req2 = Request::builder()
        .method("GET")
        .uri(&uri)
        .body(Body::empty())
        .unwrap();

    let s1 = Arc::clone(&state);
    let s2 = Arc::clone(&state);

    let t1 = tokio::spawn(async move { handle(req1, s1).await });
    let t2 = tokio::spawn(async move { handle(req2, s2).await });

    // Allow selection + increment to happen.
    sleep(Duration::from_millis(50)).await;

    {
        let backends = state.backends.lock().await;
        assert_eq!(backends.len(), 4);

        // a remains at baseline load; b and c become in-flight first.
        assert_eq!(backends[0].active_connections.load(Ordering::Relaxed), 5);
        assert_eq!(backends[1].active_connections.load(Ordering::Relaxed), 1);
        assert_eq!(backends[2].active_connections.load(Ordering::Relaxed), 1);
        assert_eq!(backends[3].active_connections.load(Ordering::Relaxed), 0);
    }

    let r1 = t1.await.unwrap().unwrap();
    let r2 = t2.await.unwrap().unwrap();

    assert_eq!(r1.status(), hyper::StatusCode::OK);
    assert_eq!(r2.status(), hyper::StatusCode::OK);

    // After both complete, counters return to seeded baseline.
    let backends = state.backends.lock().await;
    assert_eq!(backends[0].active_connections.load(Ordering::Relaxed), 5);
    assert_eq!(backends[1].active_connections.load(Ordering::Relaxed), 0);
    assert_eq!(backends[2].active_connections.load(Ordering::Relaxed), 0);
    assert_eq!(backends[3].active_connections.load(Ordering::Relaxed), 0);
}