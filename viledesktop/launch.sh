#!/usr/bin/env bash
# Start viledaemon, then vilebar, from this workspace's build output.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ -x "$ROOT/target/release/viledaemon" ]]; then
  BIN="$ROOT/target/release"
elif [[ -x "$ROOT/target/debug/viledaemon" ]]; then
  BIN="$ROOT/target/debug"
else
  echo "error: no vile binaries found under $ROOT/target/{release,debug}" >&2
  echo "build them with: cargo build --release --workspace" >&2
  exit 1
fi

for name in viledaemon vilebar; do
  if [[ ! -x "$BIN/$name" ]]; then
    echo "error: missing $BIN/$name" >&2
    exit 1
  fi
done

export RUST_LOG="${RUST_LOG:-info}"
LOGDIR="${XDG_RUNTIME_DIR:-/tmp}/vile"
mkdir -p "$LOGDIR"

running() {
  pgrep -x "$1" >/dev/null 2>&1
}

stop_one() {
  local name="$1"
  if ! running "$name"; then
    return 0
  fi
  pkill -x "$name" || true
  local i
  for i in 1 2 3 4 5; do
    running "$name" || return 0
    sleep 0.1
  done
  pkill -9 -x "$name" || true
}

start_one() {
  local name="$1"
  if running "$name"; then
    echo "$name already running"
    return 0
  fi
  setsid "$BIN/$name" >>"$LOGDIR/$name.log" 2>&1 &
  echo "started $name (pid $!)"
}

if [[ "${1:-}" == "--restart" ]]; then
  stop_one vilebar
  stop_one viledaemon
  sleep 0.15
fi

start_one viledaemon
sleep 0.2
start_one vilebar
