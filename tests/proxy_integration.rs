// full integration test

use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use hyper::{Body, Method, Request, Response, Server, service::{make_service_fn, service_fn}};
use route_iq::{
    load_balancing::{Backend, LeastConnections, RoundRobin},
    proxy::{AppState, handle},
};
use std::sync::atomic::{AtomicU64, Ordering};

// Build N backends all pointing at the same fake address. Lets a single in-test
// server stand in for the whole pool while preserving per-index counter assertions.
fn make_backends(n: usize, addr: SocketAddr) -> Vec<Backend> {
    (0..n)
        .map(|i| Backend {
            addr: format!("http://{}", addr),
            id: ((b'a' + i as u8) as char).to_string(),
            active_connections: AtomicU64::new(0),
            healthy: true,
        })
        .collect()
}


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
    // Inject a single backend that points at the in-test fake server so
    // forwarding stays self-contained (no dependency on ports 8080-8083).
    let backends = make_backends(1, backend_addr);
    let state = Arc::new(AppState::new(Box::new(RoundRobin::new()), backends));

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

    // Four backends all pointing at the same slow fake; round robin will
    // rotate a -> b -> ... so per-index counters reflect distribution.
    let backends = make_backends(4, backend_addr);
    let state = Arc::new(AppState::new(Box::new(RoundRobin::new()), backends));

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

    // Four backends all pointing at the same slow fake. We seed per-index
    // baseline load below so the LC strategy has something to choose between.
    let backends = make_backends(4, backend_addr);
    let state = Arc::new(AppState::new(Box::new(LeastConnections::new()), backends));

    // Seed baseline load so "a" is already busy and least-connections should prefer b/c first.
    {
        let backends = state.backends.lock().await;
        backends[0].active_connections.store(5, Ordering::Relaxed); // a
        backends[1].active_connections.store(2, Ordering::Relaxed); // b
        backends[2].active_connections.store(1, Ordering::Relaxed); // c
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

    // with request connections become: 5, 2, 1, 1
    let t1 = tokio::spawn(async move { handle(req1, s1).await });

    // with request connections become: 5, 2, 2, 1
    let t2 = tokio::spawn(async move { handle(req2, s2).await });

    // Allow selection + increment to happen.
    sleep(Duration::from_millis(50)).await;

    {
        let backends = state.backends.lock().await;
        assert_eq!(backends.len(), 4);

        // a remains at baseline load; c and d become in-flight first and add 1 connection
        assert_eq!(backends[0].active_connections.load(Ordering::Relaxed), 5);
        assert_eq!(backends[1].active_connections.load(Ordering::Relaxed), 2);
        assert_eq!(backends[2].active_connections.load(Ordering::Relaxed), 2);
        assert_eq!(backends[3].active_connections.load(Ordering::Relaxed), 1);
    }

    let r1 = t1.await.unwrap().unwrap();
    let r2 = t2.await.unwrap().unwrap();

    assert_eq!(r1.status(), hyper::StatusCode::OK);
    assert_eq!(r2.status(), hyper::StatusCode::OK);

    // After both complete, counters return to seeded baseline.
    let backends = state.backends.lock().await;
    assert_eq!(backends[0].active_connections.load(Ordering::Relaxed), 5);
    assert_eq!(backends[1].active_connections.load(Ordering::Relaxed), 2);
    assert_eq!(backends[2].active_connections.load(Ordering::Relaxed), 1);
    assert_eq!(backends[3].active_connections.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn proxy_returns_503_when_all_backends_are_unhealthy() {
    // No fake server needed — when every backend is marked unhealthy, the
    // strategy returns None and the proxy short-circuits to 503 *before* any
    // outbound HTTP call. So we can use bogus addresses safely.
    let backends = (0..4)
        .map(|i| Backend {
            addr: "http://127.0.0.1:1".to_string(),
            id: ((b'a' + i as u8) as char).to_string(),
            active_connections: AtomicU64::new(0),
            healthy: false,
        })
        .collect();

    let state = Arc::new(AppState::new(Box::new(RoundRobin::new()), backends));

    let req = Request::builder()
        .method("GET")
        .uri("http://example.invalid/anything")
        .body(Body::empty())
        .unwrap();

    let resp = handle(req, Arc::clone(&state)).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::SERVICE_UNAVAILABLE);

    // Counters should not have been touched — pick_backend never returned Some.
    let backends = state.backends.lock().await;
    assert!(backends.iter().all(|b| b.active_connections.load(Ordering::Relaxed) == 0));
}

#[tokio::test]
async fn admin_strategy_endpoint_swaps_balancer_at_runtime() {
    // Start the proxy with RoundRobin. We don't need a real backend running
    // for this test — the admin endpoint never forwards, it just mutates state.
    // Point at port 1 (always unbound) just to give Backend a non-empty addr.
    let backends = make_backends(2, "127.0.0.1:1".parse().unwrap());
    let state = Arc::new(AppState::new(Box::new(RoundRobin::new()), backends));

    // Sanity check: starting strategy is round_robin.
    {
        let b = state.balancer.read().await;
        assert_eq!(b.name(), "round_robin");
    }

    // POST /admin/strategy/least_connections → should swap the strategy.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/admin/strategy/least_connections")
        .body(Body::empty())
        .unwrap();
    let resp = handle(req, Arc::clone(&state)).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::OK);

    // The balancer should now be the LC implementation.
    {
        let b = state.balancer.read().await;
        assert_eq!(b.name(), "least_connections");
    }

    // POST with an unknown name → 400 Bad Request, strategy unchanged.
    let bad = Request::builder()
        .method(Method::POST)
        .uri("/admin/strategy/not_a_real_strategy")
        .body(Body::empty())
        .unwrap();
    let resp = handle(bad, Arc::clone(&state)).await.unwrap();
    assert_eq!(resp.status(), hyper::StatusCode::BAD_REQUEST);

    // Confirm the bad request did NOT overwrite the previous swap.
    let b = state.balancer.read().await;
    assert_eq!(b.name(), "least_connections");
}
