#!/usr/bin/env bash
# Real provider calls cost tokens. The isolated home retains agent/session data;
# the copied provider credentials are removed on exit.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Linux && -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]] || { echo "Linux graphical session required" >&2; exit 1; }
source_home=${PIPI_EVAL_SOURCE_HOME:-"${PIPI_HOME:-$HOME/.pipi}"}
source_agent=${PIPI_EVAL_SOURCE_AGENT:-Pipi}
eval_root=$(mktemp -d -t pipi-browser-eval.XXXXXX)
chmod 700 "$eval_root"
mkdir -p "$eval_root/home" "$eval_root/workspace" "$eval_root/results"
install -m 600 "$source_home/settings.json" "$eval_root/home/settings.json"
log_file="$eval_root/vite.log"
server_pid=
cleanup() {
  [[ -z "$server_pid" ]] || { kill "$server_pid" 2>/dev/null || true; wait "$server_pid" 2>/dev/null || true; }
  rm -f "$eval_root/home/settings.json"
}
trap cleanup EXIT
./node_modules/.bin/vite --host 127.0.0.1 --port 1420 --strictPort >"$log_file" 2>&1 &
server_pid=$!
ready=false
for _ in {1..50}; do
  if ! kill -0 "$server_pid" 2>/dev/null; then cat "$log_file" >&2; exit 1; fi
  if curl -fsS http://127.0.0.1:1420/tests/browser-eval/ >/dev/null 2>&1; then ready=true; break; fi
  sleep 0.2
done
[[ "$ready" == true ]] || { cat "$log_file" >&2; exit 1; }
echo "Evaluation data: $eval_root"
export PIPI_HOME="$eval_root/home" PIPI_EVAL_OUTPUT="$eval_root/results" PIPI_EVAL_WORKSPACE="$eval_root/workspace"
export PIPI_EVAL_MODEL_SOURCE="$source_home/agents/$source_agent/agent.json"
timeout 900s cargo run -p pipi --example browser_agent_eval --offline
