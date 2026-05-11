# route_iq

A small Rust reverse-proxy with pluggable load-balancing strategies. The goal is to internalize how a real load balancer is shaped, not to ship something production-grade.

## What it does

`route_iq` listens on `127.0.0.1:3000` and forwards every incoming HTTP request to one of N backend servers running on `127.0.0.1:8080-8083`. Which backend it picks depends on the active strategy:

- **Round Robin** — rotate through backends in order: a → b → c → d → a → ...
- **Least Connections** — pick the backend that currently has the fewest in-flight requests.

The strategy is chosen at startup in `src/main.rs`. Runtime switching via an admin endpoint is on the roadmap.

## Quick reference (copy-paste cheatsheet)

When future-you forgets every command, this section is the answer. Each block is self-contained — no setup steps to remember.

### Build

```bash
cargo build --bin backend --bin route_iq
```

> ⚠️ If the proxy is already running from a previous build, kill it before relaunching (`pkill -f target/debug/route_iq`). Otherwise the new binary silently fails to bind `:3000`.

### Start the stack (4 backends + proxy)

```bash
PORT=8080 ./target/debug/backend &
PORT=8081 ./target/debug/backend &
PORT=8082 ./target/debug/backend &
PORT=8083 ./target/debug/backend &
RUST_LOG=route_iq=info ./target/debug/route_iq
```

The proxy runs in the foreground so you can see its log output. The four backends are background jobs in the same shell.

### Stop the stack

```bash
# from the same shell that launched them:
kill $(jobs -p)

# from any shell (nuclear option):
pkill -f target/debug/backend
pkill -f target/debug/route_iq
```

### Send traffic

```bash
# Through the proxy (rotates backends per the active strategy)
curl -i http://127.0.0.1:3000/work
curl -i http://127.0.0.1:3000/health

# Directly at one backend (bypasses the proxy)
curl -i http://127.0.0.1:8080/work

# Run the Postman collection (note the path — postman CLI requires it)
postman collection run postman/collections/route_iq
```

### Intro load testing (`oha` or `hey`)

Install either `oha` (recommended) or `hey` — you only need one. They solve the same basic problem.

```bash
# from repo root
./scripts/load_test.sh

# choose hey explicitly
./scripts/load_test.sh --tool hey

# custom target / load
./scripts/load_test.sh --url http://127.0.0.1:3000/work --requests 400 --concurrency 40
```

Script location: `scripts/load_test.sh`

### Run tests

Tests are hermetic — they spawn their own in-process fake backends on ephemeral ports, so 8080–8083 don't need to be running.

```bash
cargo test                                          # all tests
cargo test -- --nocapture                           # with println output
cargo test --test backend_integration               # backend handler tests
cargo test --test proxy_integration                 # proxy / forwarding tests
RUST_LOG=route_iq=debug cargo test -- --nocapture   # tests with tracing visible
```

### Inspect what's running

```bash
lsof -nP -iTCP:3000 -iTCP:8080 -iTCP:8081 -iTCP:8082 -iTCP:8083 -sTCP:LISTEN
```

### Tracing verbosity

The proxy uses the `tracing` crate; `RUST_LOG` controls which levels print.

```bash
RUST_LOG=route_iq=warn  ./target/debug/route_iq    # only warnings/errors
RUST_LOG=route_iq=info  ./target/debug/route_iq    # routine progress (recommended)
RUST_LOG=route_iq=debug ./target/debug/route_iq    # very verbose, every probe
```

### Sample tracing output

With `RUST_LOG=route_iq=info`, a few requests + the background probe loop look like this (timestamps elided):

```
INFO route_iq: probing backends
INFO route_iq::load_balancing: probe cycle complete healthy=["a", "b", "c", "d"] unhealthy=[]

INFO handle: route_iq::proxy: selected backend=http://127.0.0.1:8080 method=GET path=/work
INFO handle: route_iq::proxy: close time.busy=414µs time.idle=15.2ms method=GET path=/work

INFO handle: route_iq::proxy: selected backend=http://127.0.0.1:8083 method=GET path=/work
INFO handle: route_iq::proxy: close time.busy=232µs time.idle=593ms method=GET path=/work

WARN route_iq::load_balancing: probe unhealthy backend=b status=500 Internal Server Error
INFO route_iq::load_balancing: probe cycle complete healthy=["a", "c", "d"] unhealthy=["b"]
```

How to read it:

- `probing backends` — the background probe loop ticking (every 2s, configured in [src/main.rs](src/main.rs)).
- `probe cycle complete healthy=[...] unhealthy=[...]` — result of one probe round.
- `handle:` prefix — every line inside that prefix fires inside the `handle` span for one specific request. `method=` and `path=` are inherited from the span, so even events from other modules (like `load_balancing::pick_backend`) get tagged with which request they belong to.
- `selected backend=...` — the strategy picked this backend at the start of the request.
- `close time.busy=... time.idle=...` — the span closed. `time.busy` is when your code was actively running; `time.idle` is when the future was parked waiting on the backend's response. For a reverse proxy, `idle` is most of the wall-clock time.
- `probe unhealthy backend=b status=500` — a probe got a 5xx from a backend, so it'll be skipped on the next `pick_backend` call until a future probe sees it healthy again.

## Architecture at a glance

```
                  ┌─────────────────────────────────┐
                  │         route_iq (proxy)        │
                  │         127.0.0.1:3000          │
  curl / Postman  │                                 │      backend (a)
        ─────────►│   1. pick backend (strategy)    │ ───► 127.0.0.1:8080
                  │   2. rewrite URI                │
                  │   3. forward via hyper::Client  │      backend (b)
                  │   4. decrement counter on ret.  │ ───► 127.0.0.1:8081
                  │                                 │
                  │                                 │      backend (c)
                  └─────────────────────────────────┘ ───► 127.0.0.1:8082

                                                          backend (d)
                                                    ───► 127.0.0.1:8083
```

Each backend is a separate OS process running the same `backend` binary, told which port to bind via the `PORT` env var. No magic — they're crash-isolated, exactly like real production hosts would be.

## Request lifecycle

Following one `GET /work` from `curl` all the way through:

```
client                proxy (:3000)                       backend (e.g. :8081)
  │                       │                                       │
  │  GET /work            │                                       │
  ├──────────────────────►│                                       │
  │                       │                                       │
  │              ┌────────┴────────┐                              │
  │              │ pick_backend()  │  picks healthy index → "b"   │
  │              └────────┬────────┘  (unhealthy entries skipped) │
  │                       │                                       │
  │             active_connections[b] += 1                        │
  │                       │                                       │
  │           rewrite URI → http://127.0.0.1:8081/work            │
  │                       │                                       │
  │                       │  GET /work                            │
  │                       ├──────────────────────────────────────►│
  │                       │                                       │
  │                       │                       does ~10ms work │
  │                       │                                       │
  │                       │             200 OK (or 5xx, see note) │
  │                       │◄──────────────────────────────────────┤
  │                       │                                       │
  │             active_connections[b] -= 1                        │
  │                       │                                       │
  │  passthrough status   │                                       │
  │◄──────────────────────┤                                       │
```

**Notes worth remembering:**

- The strategy never holds the mutex while the backend call is in flight — the proxy locks just long enough to pick + increment, releases, forwards, then re-locks briefly to decrement. That's why concurrent requests can run in parallel even though the backend list lives behind a `Mutex`.
- The `healthy` flag on each backend is set by a **separate background probe task** that runs every 2 seconds (see `tokio::spawn` in [src/main.rs](src/main.rs)). The request path only *reads* it — health checks never block traffic.
- The proxy is intentionally **passthrough**: it does not retry, rewrite responses, or hide upstream errors. If the backend returns 500/503/504, the client sees that same status. The strategies bias *which* backend gets traffic, not whether the response succeeds.

## Project layout

```
src/
├── main.rs                    # proxy entry point: backend list, tracing init, probe loop, hyper Server on :3000
├── lib.rs                     # exports the modules below
├── proxy.rs                   # AppState, handle() — the proxy request handler
├── load_balancing.rs          # Backend struct, LoadBalancingStrategy trait, RR / LC impls, HttpHealthCheck
├── backend.rs                 # backend handlers: handle / health / work (reusable from tests + bin)
└── bin/
    └── backend.rs             # thin tokio::main wrapper that serves route_iq::backend::handle

tests/
├── proxy_integration.rs       # end-to-end proxy tests with in-process fake backends
└── backend_integration.rs     # backend handler tests (calls handle directly, no subprocess)

postman/
└── collections/route_iq       # YAML collection for `postman collection run postman/collections/route_iq`
```

Quick map of "where does X live":

| You want to change... | Look in |
|----------------------|---------|
| The list of backend addresses | [src/main.rs](src/main.rs) |
| How often health probes run | [src/main.rs](src/main.rs) (`tokio::spawn` block) |
| How a request gets forwarded | [src/proxy.rs](src/proxy.rs) (`handle`) |
| The selection algorithm | [src/load_balancing.rs](src/load_balancing.rs) |
| The real `/health` HTTP probe | [src/load_balancing.rs](src/load_balancing.rs) (`HttpHealthCheck::is_healthy`) |
| Backend route handlers (`/health`, `/work`, 404) | [src/backend.rs](src/backend.rs) |
| Proxy integration tests | [tests/proxy_integration.rs](tests/proxy_integration.rs) |
| Backend handler tests | [tests/backend_integration.rs](tests/backend_integration.rs) |

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
- Hermetic integration tests with injected backend lists
- `PORT`-configurable backend binary, runs N copies as separate processes
- Postman collection runnable from the terminal
- Real `/health` and `/work` endpoints on the backend (with built-in randomized latency / error injection on `/work` so RR vs LC actually diverge under load)
- Real `HttpHealthCheck` — issues an HTTP probe to each backend's `/health` and reads `status().is_success()`
- Background health-check task on a 2-second tick (probe lives outside the request hot path; `handle` no longer locks for health)
- `tracing` instrumentation with `RUST_LOG`-driven filtering — per-probe results, probe-cycle summary, and per-strategy "skipping unhealthy" warnings
- Per-request `#[tracing::instrument]` span on `proxy::handle` — events from `load_balancing` (probe, skip) and `proxy` (selected, close) all inherit `method` and `path` fields, so concurrent requests are no longer interleaved noise
- Span close events emit `time.busy` / `time.idle` automatically via `FmtSpan::CLOSE` — per-request latency observability without any per-handler timing code

**Next (week 2 finishing)**
- `POST /admin/strategy` endpoint to swap strategies at runtime
- Compare RR vs LC behavior under load using `hey` / `oha` and capture latency percentiles
- Decide whether to expose `?delay_ms=` / `?error_rate=` query knobs on `/work` for repeatable scenarios (currently the variation is randomized inside the handler — fine for ambient noise, but harder to reproduce a specific stress condition)

**Week 3**
- Decision engine: collect per-backend latency / error metrics in-memory, auto-switch strategies based on triggered conditions (e.g. p95 latency above threshold)
- Rate-limit algorithm switches (at most one per 60s window)

**Optional / week 4+**
- External observability (Datadog, etc.)
- Fault tolerance for proxy failures
- Containerization + orchestration

**Stretch direction: AI inference routing**

The same proxy shape applies to routing across LLMs (GPT, Claude, Llama, fine-tuned small models) — but the "backends" have wildly different cost, latency, and capability, so selection gets smarter:

- *Cost-based* — try cheap model first, escalate only if needed.
- *Capability-based* — classify the request (code / math / chat) and route to the best-fit model.
- *Cascading* — small model answers first; retry on a bigger one if confidence is low.
- *Semantic caching* — hash prompt meaning, return cached answer for similar queries.
- *Token-aware least-loaded* — track tokens-in-flight per backend (a 100k-token request ties up a GPU very differently from a 100-token one).

This is the "AI gateway" product category (Portkey, LiteLLM, Cloudflare AI Gateway).

**Stretch direction: security & red teaming**

Layered on top of the AI-gateway shape:

- *Prompt injection / jailbreak detection* on inbound requests.
- *Output filtering* on model responses before returning to client.
- *PII redaction* before sending to third-party providers.
- *Per-user / per-key rate limits* — relevant when each request costs real money.
- *Red teaming* — adversarial testing where attackers (humans or automated) actively try to extract secrets, jailbreak the model, or trigger harmful outputs. Build a harness that runs known attack prompts against the gateway and asserts they're blocked.

## Implementation notes

- `active_connections` is tracked with atomics (`AtomicU64`) and updated with `fetch_add` / `fetch_sub`.
- `Ordering::Relaxed` is used for counters where atomicity is required but cross-variable memory ordering is not.
- `tokio::sync::Mutex` is held only briefly during backend selection, never across the forward I/O.

## References

- [Hyper Reverse Proxy Example](https://github.com/monroeclinton/hyper-learn/tree/main)
- [Accompanying YouTube video](https://www.youtube.com/watch?v=cICaUDqZ5t0)
