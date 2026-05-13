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

### Swap load-balancing strategy at runtime

```bash
curl -X POST http://127.0.0.1:3000/admin/strategy/round_robin
curl -X POST http://127.0.0.1:3000/admin/strategy/least_connections
curl -X POST http://127.0.0.1:3000/admin/strategy/garbage     # → 400 Bad Request
```

No proxy restart needed. Subsequent `/work` requests will use the newly selected strategy.

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

The starting `LoadBalancingStrategy` is set at startup in [src/main.rs](src/main.rs):

```rust
let state = Arc::new(AppState::new(Box::new(RoundRobin::new()), backends));
//                                          ^^^^^^^^^^^^^^^ swap to LeastConnections::new()
```

After startup, the strategy can be swapped at runtime via `POST /admin/strategy/<name>` (see cheatsheet). The balancer lives behind a `RwLock<Box<dyn LoadBalancingStrategy>>` in `AppState`, so the swap is atomic — in-flight requests finish on the old strategy, new requests use the new one.

## Load testing with `oha`

Once the stack is up, you can drive synthetic traffic at the proxy to see how the strategies behave under load. [`oha`](https://github.com/hatoo/oha) is a small Rust load tester with a live histogram TUI; install with `brew install oha`.

A smoke test (1000 requests, 10 concurrent):

```bash
oha -n 1000 -c 10 http://127.0.0.1:3000/work
```

Aim at `/work` — it has built-in randomized delay and error injection, so the numbers actually exercise the system. `/health` is too fast to measure usefully.

### Reading the output

A real run against this stack:

```
Summary:
  Success rate: 100.00%
  Total:        754.1913 ms
  Slowest:      737.4475 ms
  Fastest:      8.7268 ms
  Average:      28.3388 ms
  Requests/sec: 132.5923

Response time histogram:
    8.727 ms [ 1] |
   81.599 ms [96] |■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■
  154.471 ms [ 0] |
  ...
  445.959 ms [ 2] |
  ...
  737.447 ms [ 1] |

Response time distribution:
  50.00% in  13.1140 ms      ← p50: typical request
  95.00% in  17.8680 ms      ← p95: most users' worst experience
  99.00% in 737.4475 ms      ← p99: the slow-path 1%

Status code distribution:
  [200] 92 responses
  [500]  5 responses
  [503]  3 responses
```

What each block tells you:

- **Success rate** — % of requests that got *any* response back, **not** HTTP 2xx. A 500 or 503 still counts as "successful" here; this only drops when a request times out or the connection is refused.
- **Slowest / Fastest / Average** — average is misleading when latency is bimodal (most fast, a few very slow). The percentiles below are what you actually want to look at.
- **Requests/sec** — throughput. Use for comparison between runs, not as an absolute SLA.
- **Response time histogram** — visual of where requests landed. In this run, 96 of 100 sat in the 8–82ms bucket (the normal `/work` path), with 3 outliers in the 446–737ms range (the 5% random slow path in `backend.rs` firing).
- **Response time distribution (percentiles)** — the most useful section:
  - **p50 (13ms)** — median. Half the requests were faster, half were slower.
  - **p95 (18ms)** — only 5% of requests were slower than this.
  - **p99 (737ms)** — the tail. This is **where strategies diverge most.** Only 1% of requests hit a slow backend, but when they did, they took 700+ms.
- **Status code distribution** — your `/work` handler randomly returns 500 (~6%) and 503 (~3%) on purpose to simulate flaky upstream. Here 5×500 + 3×503 + 92×200 matches the design.

### Comparing Round Robin vs Least Connections

Swap strategies between runs without restarting:

```bash
curl -X POST http://127.0.0.1:3000/admin/strategy/round_robin
oha -n 5000 -c 50 http://127.0.0.1:3000/work        # capture this run

curl -X POST http://127.0.0.1:3000/admin/strategy/least_connections
oha -n 5000 -c 50 http://127.0.0.1:3000/work        # capture this run
```

Save both `oha` outputs and compare:

| Metric | What to watch for |
|--------|-------------------|
| **p50** | Usually unchanged — typical latency isn't strategy-sensitive. |
| **p95** | Subtle differences. |
| **p99** | **The headline number.** LC should meaningfully beat RR here when one backend is slow, because LC stops piling new requests onto a backend that's already stuck. |
| **Status distribution** | Sanity check — same `/work` randomness, so similar 200/500/503 ratios across runs. |

### Per-backend distribution histogram

`oha` only sees the proxy's response, not which backend handled each request. To see the actual distribution, capture the proxy's tracing output and tally the `selected backend=` events:

```bash
# Launch the proxy with output captured to a file in the repo:
RUST_LOG=route_iq=info ./target/debug/route_iq 2>&1 | tee logs/proxy.log

# In another terminal, run your oha load test against the proxy, then:
sed -E 's/\x1b\[[0-9;]*m//g' logs/proxy.log \
  | grep "selected backend=" \
  | awk -F'backend=' '{print $2}' \
  | awk '{print $1}' \
  | sort | uniq -c
```

The `logs/` directory is tracked (via `.gitkeep`) so anyone cloning the repo gets the folder. `logs/proxy.log` itself is gitignored — it's the active working file, overwritten every run. If you want to preserve a particular run for the repo, copy it to a named file (e.g. `cp logs/proxy.log logs/sample-rr-2026-05-13.log`) and commit that explicitly. Named samples are *not* gitignored.

Sample output for a 1000-request run under Least Connections (note the `a` backend got fewer hits — LC was biasing away from it):

```
 192 http://127.0.0.1:8080
 270 http://127.0.0.1:8081
 269 http://127.0.0.1:8082
 269 http://127.0.0.1:8083
```

Under pure Round Robin against equal backends you'd expect ~250/250/250/250. Deviation from that = the strategy doing its job.

> 🪲 The `sed` step strips ANSI color codes that `tracing-subscriber` writes when stdout is connected to a terminal (via `tee`). To skip the `sed` step permanently, add `.with_ansi(false)` to the `tracing_subscriber::fmt()` builder in [src/main.rs](src/main.rs) — the log file will then be plain text on disk.

For a clean signal you'll want more volume than the smoke test — try `oha -n 5000 -c 50` and run with `RUST_LOG=route_iq=info` so the `selected` events still land in the log file. Use `RUST_LOG=route_iq=warn` instead if you don't need the per-request distribution and just want quieter logs.

## Benchmarks

The whole load-test-and-compare-strategies dance lives in one script: [benchmarks/run.sh](benchmarks/run.sh). One command, end-to-end.

### How to run

1. Start the stack with the proxy's stdout tee'd to `logs/proxy.log` (the script needs it for the per-backend histogram):

   ```bash
   PORT=8080 ./target/debug/backend &
   PORT=8081 ./target/debug/backend &
   PORT=8082 ./target/debug/backend &
   PORT=8083 ./target/debug/backend &
   RUST_LOG=route_iq=info ./target/debug/route_iq 2>&1 | tee logs/proxy.log
   ```

2. In another terminal, run the benchmark:

   ```bash
   ./benchmarks/run.sh
   # or override the defaults (N=5000, C=50):
   N=2000 C=20 ./benchmarks/run.sh
   ```

The script preflight-checks `oha`, the proxy, and `logs/proxy.log`, then:

- Switches the proxy to **Round Robin** via `POST /admin/strategy/round_robin`
- Runs `oha -n N -c C --no-tui` against `/work`, saving the full output
- Tallies the per-backend distribution from `logs/proxy.log` (only this run's events)
- Repeats for **Least Connections**
- Prints a summary to stdout *and* `benchmarks/results/<timestamp>/summary.txt`

### What gets saved

```
benchmarks/results/<timestamp>/
├── rr-oha.txt                 # full oha output for Round Robin
├── rr-distribution.txt        # per-backend selection counts (RR)
├── lc-oha.txt                 # full oha output for Least Connections
├── lc-distribution.txt        # per-backend selection counts (LC)
└── summary.txt                # combined headline metrics
```

`benchmarks/results/` is **gitignored by default** so re-runs don't pollute `git status`. To preserve a particular run as the repo's canonical reference, force-add it:

```bash
git add -f benchmarks/results/<timestamp>
git commit -m "benchmarks: <timestamp> RR vs LC reference run"
```

### Reading the comparison

The headline metrics to look at, side by side:

| Section | What to compare |
|---------|-----------------|
| **Percentiles (p50 / p95 / p99)** | p99 is where strategies diverge most. LC should beat RR if any backend is slower than the others. |
| **Status codes** | Should be ~similar across both runs — same `/work` randomness exercising both. |
| **Per-backend distribution** | RR ≈ flat (e.g. 1250/1250/1250/1250 for N=5000). LC will skew if backends drift in load. |

### Reproducibility for someone cloning the repo

If a committed reference run exists under `benchmarks/results/<timestamp>/`, anyone can:

1. Clone, `cargo build`, start the stack as above.
2. Run `./benchmarks/run.sh`.
3. Diff their new run's `summary.txt` against the reference to see if their setup behaves the same.

Numbers won't match exactly — different hardware, different background processes — but the *shape* (RR even, LC biased, similar status-code ratio) should reproduce.

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
- Load-test harness wired up with `oha` — smoke run confirmed against `/work`, README documents how to install, run, and interpret the output (percentiles, histogram, status distribution)
- `AppState` refactored from generic `<B: LoadBalancingStrategy>` to a non-generic struct with `balancer: RwLock<Box<dyn LoadBalancingStrategy>>` — strategy can be swapped at runtime through the lock without restarting the proxy
- `POST /admin/strategy/<name>` endpoint — accepts `round_robin` or `least_connections`, returns 400 on unknown names, and is unit-tested for both the happy path and the bad-name path
- Per-backend distribution histogram pipeline documented (`sed | grep | awk | sort | uniq -c` over the proxy log) — lets you visually confirm RR vs LC actually picks different backends under the same workload

**Next (week 2 finishing)**
- Run a full RR vs LC head-to-head with `oha` (e.g. `-n 5000 -c 50`), capture both runs' p50 / p95 / p99 + status distribution + per-backend histogram, and add a `## Benchmarks` section to this README with the numbers
- Update the Postman collection's two `(Planned)` admin requests to use the new path-based URLs (`/admin/strategy/round_robin`, `/admin/strategy/least_connections`) so the whole collection runs clean
- Decide whether to expose `?delay_ms=` / `?error_rate=` query knobs on `/work` for repeatable scenarios — randomized injection is fine for ambient noise but harder to reproduce a specific stress condition. Punt this decision until after the first head-to-head: if the random variation produces a clear RR vs LC signal, no need to add knobs.

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
