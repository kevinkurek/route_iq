#!/usr/bin/env bash
# Run an RR vs LC head-to-head against the running proxy.
#
# Prerequisites:
#   1. Stack is running:  4 backends on :8080-:8083, proxy on :3000
#   2. Proxy was launched with stdout tee'd to logs/proxy.log, e.g.:
#        RUST_LOG=route_iq=info ./target/debug/route_iq 2>&1 | tee logs/proxy.log
#   3. `oha` is installed:  brew install oha
#
# Usage:
#   ./benchmarks/run.sh                 # defaults: N=5000, C=50
#   N=2000 C=20 ./benchmarks/run.sh     # override request count / concurrency
#
# Output lands in benchmarks/results/<timestamp>/ — that directory is
# gitignored by default. Force-add a run to commit it as a reference.

set -euo pipefail

N=${N:-5000}
C=${C:-50}
TARGET=${TARGET:-http://127.0.0.1:3000/work}
LOG=logs/proxy.log
TS=$(date +%Y-%m-%d-%H%M%S)
OUT=benchmarks/results/$TS

# ---------- preflight ----------

command -v oha >/dev/null 2>&1 || {
    echo "❌ oha not found. Install: brew install oha" >&2
    exit 1
}

curl -sf http://127.0.0.1:3000/health >/dev/null || {
    echo "❌ proxy not responding on http://127.0.0.1:3000" >&2
    echo "   start the stack first (see README 'Quick reference')." >&2
    exit 1
}

[[ -f "$LOG" ]] || {
    echo "❌ $LOG not found." >&2
    echo "   relaunch the proxy with output tee'd to it:" >&2
    echo "     RUST_LOG=route_iq=info ./target/debug/route_iq 2>&1 | tee $LOG" >&2
    exit 1
}

mkdir -p "$OUT"
echo "==> benchmark run: $TS"
echo "    N=$N  C=$C  TARGET=$TARGET"
echo "    results: $OUT"
echo ""

# ---------- one strategy ----------

run_one() {
    local strategy=$1     # round_robin | least_connections
    local label=$2        # short tag for filenames: rr | lc

    echo "--- switching to $strategy ---"
    curl -fsS -X POST "http://127.0.0.1:3000/admin/strategy/$strategy" >/dev/null
    sleep 1   # let any in-flight requests drain on the previous strategy

    # Mark log byte offset before this run so we only count its events.
    local before_lines
    before_lines=$(wc -l < "$LOG")

    echo "running: oha -n $N -c $C --no-tui $TARGET"
    oha -n "$N" -c "$C" --no-tui "$TARGET" > "$OUT/$label-oha.txt"

    # Wait for tee/buffer flush; tracing events may lag actual responses by ms.
    sleep 2

    # Extract this run's selections from the log and tally per-backend.
    tail -n +"$((before_lines + 1))" "$LOG" \
        | sed -E 's/\x1b\[[0-9;]*m//g' \
        | grep "selected backend=" \
        | awk -F'backend=' '{print $2}' \
        | awk '{print $1}' \
        | sort | uniq -c > "$OUT/$label-distribution.txt"

    echo "done ($label)"
    echo ""
}

run_one round_robin       rr
run_one least_connections lc

# ---------- summary ----------

summarize() {
    local label=$1
    local pretty=$2
    echo "=== $pretty ($label) ==="
    grep -E "Success rate|Requests/sec|Slowest|Fastest|Average" "$OUT/$label-oha.txt" \
        | sed 's/^[[:space:]]*//'
    echo ""
    echo "Percentiles:"
    grep -E "^  (50|90|95|99)\.00%" "$OUT/$label-oha.txt"
    echo ""
    echo "Status codes:"
    awk '/Status code distribution:/{flag=1; next} /^$/{flag=0} flag' "$OUT/$label-oha.txt" \
        | sed 's/^[[:space:]]*//'
    echo ""
    echo "Per-backend selections:"
    cat "$OUT/$label-distribution.txt"
    echo ""
}

{
    echo "Benchmark run: $TS"
    echo "Config: oha -n $N -c $C --no-tui $TARGET"
    echo ""
    summarize rr "Round Robin"
    summarize lc "Least Connections"
} | tee "$OUT/summary.txt"

echo ""
echo "✅ Done. To commit this run as a reference:"
echo "    git add -f $OUT && git commit -m \"benchmarks: $TS RR vs LC reference run\""
