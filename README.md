# route_iq

A small Rust reverse-proxy with pluggable load-balancing strategies. The goal is to internalize how a real load balancer is shaped, not to ship something production-grade.

## What it does

`route_iq` listens on `127.0.0.1:3000` and forwards every incoming HTTP request to one of N backend servers running on `127.0.0.1:8080-8083`. Which backend it picks depends on the active strategy:

- **Round Robin** — rotate through backends in order: a → b → c → d → a → ...
- **Least Connections** — pick the backend that currently has the fewest in-flight requests.

The strategy is chosen at startup in `src/main.rs`. Runtime switching via an admin endpoint is on the roadmap.

## Architecture at a glance

```
                  ┌─────────────────────────────────┐
                  │         route_iq (proxy)        │
                  │         127.0.0.1:3000          │
  curl / Postman  │                                 │      backend (a)
        ─────────►│   1. log middleware             │ ───► 127.0.0.1:8080
                  │   2. pick backend (strategy)    │
                  │   3. rewrite URI                │      backend (b)
                  │   4. forward via hyper::Client  │ ───► 127.0.0.1:8081
                  │   5. decrement counter on ret.  │
                  │                                 │      backend (c)
                  └─────────────────────────────────┘ ───► 127.0.0.1:8082

                                                          backend (d)
                                                    ───► 127.0.0.1:8083
```

Each backend is a separate OS process running the same `backend` binary, told which port to bind via the `PORT` env var. No magic — they're crash-isolated, exactly like real production hosts would be.

## Request lifecycle

Following one `GET /hello` from `curl` all the way through:

```
client                proxy (:3000)                       backend (e.g. :8081)
  │                       │                                       │
  │  GET /hello           │                                       │
  ├──────────────────────►│                                       │
  │                       │                                       │
  │                  ┌────┴────┐                                  │
  │                  │ log mw  │  prints "Generic Path: /hello"   │
  │                  └────┬────┘                                  │
  │                       │                                       │
  │              ┌────────┴────────┐                              │
  │              │ pick_backend()  │  strategy picks index → "b"  │
  │              └────────┬────────┘                              │
  │                       │                                       │
  │             active_connections[b] += 1                        │
  │                       │                                       │
  │           rewrite URI → http://127.0.0.1:8081/hello           │
  │                       │                                       │
  │                       │  GET /hello                           │
  │                       ├──────────────────────────────────────►│
  │                       │                                       │
  │                       │                              200 OK   │
  │                       │◄──────────────────────────────────────┤
  │                       │                                       │
  │             active_connections[b] -= 1                        │
  │                       │                                       │
  │              200 OK   │                                       │
  │◄──────────────────────┤                                       │
```

The strategy never holds the mutex while the backend call is in flight — the proxy locks just long enough to pick + increment, releases, forwards, then re-locks briefly to decrement. That's why concurrent requests can run in parallel even though the backend list lives behind a `Mutex`.

## Project layout

```
src/
├── main.rs              # entry point: defines backend list, starts hyper Server on :3000
├── lib.rs               # exports the three modules below
├── proxy.rs             # AppState, handle() — the request handler
├── middleware.rs        # log() — wraps handle() to print path classification
├── load_balancing.rs    # Backend struct, LoadBalancingStrategy trait, RR / LC impls
└── bin/
    └── backend.rs       # tiny stand-alone backend; reads PORT, returns 200 to anything

tests/
└── proxy_integration.rs # end-to-end tests with in-process fake backends

postman/
└── collections/route_iq # YAML collection for `postman collection run`
```

Quick map of "where does X live":

| You want to change... | Look in |
|----------------------|---------|
| The list of backend addresses | [src/main.rs](src/main.rs) |
| How a request gets forwarded | [src/proxy.rs](src/proxy.rs) (`handle`) |
| The selection algorithm | [src/load_balancing.rs](src/load_balancing.rs) |
| What gets logged per request | [src/middleware.rs](src/middleware.rs) |
| The backend's own behavior (currently always-200) | [src/bin/backend.rs](src/bin/backend.rs) |
| End-to-end tests | [tests/proxy_integration.rs](tests/proxy_integration.rs) |

## Run the stack

### 1. Build both binaries

```bash
cargo build --bin backend --bin route_iq
```

### 2. Start four backend instances

The backend reads `PORT` from the environment (defaults to `8080`). Run four copies on different ports:

```bash
cargo build --bin backend
PORT=8080 ./target/debug/backend &
PORT=8081 ./target/debug/backend &
PORT=8082 ./target/debug/backend &
PORT=8083 ./target/debug/backend &

# kill all four at once:
kill $(jobs -p)
```

Each one prints `HTTP server listening on http://127.0.0.1:808X` once it's ready.

### 3. Start the proxy

In another terminal:

```bash
./target/debug/route_iq
# or: cargo run --bin route_iq
```

> ⚠️ When you change Rust code, kill the proxy (`pkill -f target/debug/route_iq`) before relaunching. Otherwise the OS keeps the old binary on :3000 and silently rejects the new one.

### 4. Send traffic

```bash
# Through the proxy — rotates across backends per the active strategy
curl -i http://127.0.0.1:3000/hello

# Directly at one backend (bypassing the proxy)
curl -i http://127.0.0.1:8080/
```

Watch the proxy terminal — you'll see `Selected backend: http://127.0.0.1:808X` for every request, which is how you visually confirm the strategy is rotating.

### 5. Run the Postman collection from the terminal

```bash
postman collection run postman/collections/route_iq
```

All routes should return 200 once the four backends are running.

## Test

```bash
cargo test                                              # everything
cargo test -- --nocapture                               # with println output
cargo test --test proxy_integration -- --nocapture      # integration only
```

Integration tests are hermetic — they spawn their own in-process fake backend on an ephemeral port, so they don't depend on anything listening on 8080–8083.

## Load-balancing strategies

Both implement the same trait, so the rest of the system doesn't know which one is in use:

```rust
pub trait LoadBalancingStrategy: Send + Sync {
    fn pick_backend(&self, backends: &[Backend]) -> Option<usize>;
    fn name(&self) -> &'static str;
}
```

### Round Robin
Maintains an atomic counter; picks `healthy[counter % healthy.len()]`. Filters unhealthy backends out of the rotation. Predictable, no per-backend state needed.

### Least Connections
Scans all healthy backends and returns the one with the smallest `active_connections` counter. Counters are `AtomicU64`s on each `Backend`, incremented just before forwarding and decremented when the response returns.

The `LoadBalancingStrategy` choice is currently set at startup in [src/main.rs](src/main.rs):

```rust
let state = Arc::new(AppState::new(RoundRobin::new(), backends));
//                                  ^^^^^^^^^^^^^^^ swap to LeastConnections::new()
```

## Status & roadmap

**Done**
- Reverse-proxy core: hyper server, URI rewrite, forward via `hyper::Client`
- Round Robin and Least Connections strategies behind a shared trait
- Per-backend in-flight counter using atomics
- Path-classifying logging middleware (`/api/*` vs everything else)
- Hermetic integration tests with injected backend lists
- `PORT`-configurable backend binary, runs N copies as separate processes
- Postman collection runnable from the terminal

**Next (week 2 finishing)**
- Real `/health` and `/work` endpoints on the backend (currently returns 200 to any path)
- `?delay_ms=` and `?error_rate=` query knobs on `/work` for stress-testing
- `POST /admin/strategy` endpoint to swap strategies at runtime
- Compare RR vs LC behavior under load using `hey` / `oha`

**Week 3**
- Real `HttpHealthCheck` (currently a stub that always returns the cached `healthy` flag)
- Background health-check task — current code probes inline on every request, which won't scale
- Decision engine: collect per-backend latency / error metrics, auto-switch strategies based on conditions

**Optional / week 4+**
- External observability (Datadog, etc.)
- Fault tolerance for proxy failures
- Containerization + orchestration

## Implementation notes

- `active_connections` is tracked with atomics (`AtomicU64`) and updated with `fetch_add` / `fetch_sub`.
- `Ordering::Relaxed` is used for counters where atomicity is required but cross-variable memory ordering is not.
- `tokio::sync::Mutex` is held only briefly during backend selection, never across the forward I/O.

## References

- [Hyper Reverse Proxy Example](https://github.com/monroeclinton/hyper-learn/tree/main)
- [Accompanying YouTube video](https://www.youtube.com/watch?v=cICaUDqZ5t0)
