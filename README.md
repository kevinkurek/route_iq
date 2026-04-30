# route_iq

A small Rust reverse-proxy with pluggable load balancing strategies.

Current strategies:
- Round Robin
- Least Connections

## Run The Stack

The proxy listens on `127.0.0.1:3000` and forwards to backends on `127.0.0.1:8080-8083`.

### 1. Build the backend binary once

```bash
cargo build --bin backend
```

### 2. Start four backend instances

The backend reads `PORT` from the environment (defaults to `8080`). Run four copies, each on a different port. Easiest is one terminal tab per backend. Background them all in a single shell:

```bash
cargo build --bin backend
PORT=8080 ./target/debug/backend &
PORT=8081 ./target/debug/backend &
PORT=8082 ./target/debug/backend &
PORT=8083 ./target/debug/backend &

# kill with
kill $(jobs -p)
```

(Use `jobs` to list, `kill %1 %2 %3 %4` to stop them.)

### 3. Start the proxy

In another terminal:

```bash
cargo run --bin route_iq
```

### 4. Send traffic

Hit a backend directly:

```bash
curl -i http://127.0.0.1:8080/
```

Hit the proxy — it rewrites the URI and forwards to one of the four backends per the active strategy:

```bash
curl -i http://127.0.0.1:3000/hello
```

Watch the proxy terminal — `Selected backend: http://127.0.0.1:808X` logs which backend was chosen.

### 5. Run the Postman collection from the terminal

```bash
postman collection run postman/collections/route_iq
```

All proxy routes should return 200 once the four backends are running.

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
