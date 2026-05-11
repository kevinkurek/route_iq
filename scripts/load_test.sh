#!/usr/bin/env bash
set -euo pipefail

# Intro load-test helper for route_iq.
# Uses oha by default, or hey if requested / available.
#
# Examples:
#   ./scripts/load_test.sh
#   ./scripts/load_test.sh --tool hey
#   ./scripts/load_test.sh --url http://127.0.0.1:3000/work --requests 400 --concurrency 40
#
# Prereqs:
#   - route_iq stack running (proxy on :3000)
#   - one of: oha, hey

TOOL="auto"
URL="http://127.0.0.1:3000/work"
REQUESTS=200
CONCURRENCY=20

usage() {
  cat <<USAGE
Usage: $0 [--tool auto|oha|hey] [--url URL] [--requests N] [--concurrency N]

Defaults:
  --tool auto
  --url http://127.0.0.1:3000/work
  --requests 200
  --concurrency 20
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tool)
      TOOL="${2:-}"
      shift 2
      ;;
    --url)
      URL="${2:-}"
      shift 2
      ;;
    --requests)
      REQUESTS="${2:-}"
      shift 2
      ;;
    --concurrency)
      CONCURRENCY="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

select_tool() {
  if [[ "$TOOL" == "oha" || "$TOOL" == "hey" ]]; then
    echo "$TOOL"
    return 0
  fi

  if command -v oha >/dev/null 2>&1; then
    echo "oha"
    return 0
  fi

  if command -v hey >/dev/null 2>&1; then
    echo "hey"
    return 0
  fi

  echo "error"
}

SELECTED_TOOL="$(select_tool)"

if [[ "$SELECTED_TOOL" == "error" ]]; then
  cat >&2 <<ERR
No load-test tool found.
Install one of:
  - oha  (recommended)
  - hey
ERR
  exit 1
fi

echo "Running load test with: $SELECTED_TOOL"
echo "Target: $URL"
echo "Requests: $REQUESTS"
echo "Concurrency: $CONCURRENCY"

if [[ "$SELECTED_TOOL" == "oha" ]]; then
  exec oha -n "$REQUESTS" -c "$CONCURRENCY" "$URL"
fi

exec hey -n "$REQUESTS" -c "$CONCURRENCY" "$URL"
