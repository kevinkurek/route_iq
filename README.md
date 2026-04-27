# route_iq

A small Rust reverse-proxy with pluggable load balancing strategies.

Current strategies:
- Round Robin
- Least Connections

## Run The Proxy

Starts the proxy on `127.0.0.1:3000`.

```bash
cargo run --bin route_iq
```

Then in another terminal start the backend server and test both direct and proxied requests.

This starts the standalone example server on `127.0.0.1:8080`.

```bash
cargo run --bin basic_http_8080
```

Test the backend directly:

```bash
curl -i http://127.0.0.1:8080
>>
HTTP/1.1 200 OK
Hello from route_iq binary on :8080
```

Test through the proxy (port 3000) to backend (port 8080):

```bash
curl -i --proxy http://127.0.0.1:3000 http://127.0.0.1:8080/
>>
HTTP/1.1 200 OK
Hello from route_iq binary on :8080
```

Short `-x` equivalent:

```bash
curl -i -x 127.0.0.1:3000 http://127.0.0.1:8080/

```

## Run Tests

Run everything:

```bash
cargo test
```

Run with logs:

```bash
cargo test -- --nocapture
```

Run only integration tests:

```bash
cargo test --test proxy_integration -- --nocapture
```

## Load Balancing Strategy Examples

The strategy is chosen in `src/main.rs` when creating app state.

### Round Robin (default)

```bash
# state example:
# let state = Arc::new(AppState::new(RoundRobin::new()));

cargo run
```

### TODO:
- Implement the ability to switch algorithms at runtime via an endpoint on the load balancer.
- Update the work endpoint to artificially introduce conditions like prolonged request processing and elevated error rates.
- Evaluate the performance impact of different load balancing algorithms under various conditions.
- For now, collect these metrics about worker servers within the load balancer using simple in-memory data structures (e.g., HashMap). No need to use third-party services like Datadog.
- Implement production tracing
- 

## References
- [Hyper Reverse Proxy Example](https://github.com/monroeclinton/hyper-learn/tree/main)
- [Accompanying Youtube Video](https://www.youtube.com/watch?v=cICaUDqZ5t0)

## Notes

- `active_connections` is tracked with atomics (`AtomicU64`) and updated with `fetch_add` / `fetch_sub`.
- `Ordering::Relaxed` is used for counters where atomicity is required but cross-variable memory ordering is not.
