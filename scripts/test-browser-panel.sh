#!/usr/bin/env bash
# Run the Linux desktop browser probe against its local fixture.
set -euo pipefail

cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Linux || -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  echo "This test requires a Linux graphical session." >&2
  exit 1
fi

log_file=$(mktemp -t pipi-browser-panel.XXXXXX)
./node_modules/.bin/vite --host 127.0.0.1 --port 1420 --strictPort >"$log_file" 2>&1 &
server_pid=$!
cleanup() {
  kill "$server_pid" 2>/dev/null || true
  wait "$server_pid" 2>/dev/null || true
  rm -f "$log_file"
}
trap cleanup EXIT

ready=false
for _ in {1..50}; do
  if ! kill -0 "$server_pid" 2>/dev/null; then
    cat "$log_file" >&2
    exit 1
  fi
  if grep -q 'ready in' "$log_file" && curl -fsS http://127.0.0.1:1420/tests/browser-panel.browser.html >/dev/null 2>&1; then
    ready=true
    break
  fi
  sleep 0.2
done
if [[ "$ready" != true ]]; then
  cat "$log_file" >&2
  echo "Browser fixture did not start." >&2
  exit 1
fi

PIPI_TEST_LAYOUT_FLOOD=1 timeout 180s cargo run -p pipi --example browser_panel_probe --offline
