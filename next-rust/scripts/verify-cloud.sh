#!/usr/bin/env bash
# Deterministic Linux validation. Never installs dependencies or contacts real providers.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:-}" in
 --help) echo 'Usage: verify-cloud.sh. Offline Linux core/web tests and builds; macOS/Tauri/native picker are NOT checked.'; exit 0;;
 "") ;;
 *) echo 'Unknown option. Live provider checks must be explicitly separate.' >&2; exit 2;;
esac
[[ "$(uname -s)" == Linux ]] || { echo 'Use verify-local.sh for native macOS verification.' >&2; exit 1; }
if command -v cargo >/dev/null; then CARGO="$(command -v cargo)"
elif [[ -x "$HOME/.cargo/bin/cargo" ]]; then CARGO="$HOME/.cargo/bin/cargo"
else echo 'Missing Rust toolchain. Install it separately from the official vendor.' >&2; exit 1; fi
[[ -d "$ROOT/web/node_modules" ]] || { echo 'Missing web dependencies. Run npm ci separately.' >&2; exit 1; }
TMP="$(mktemp -d "${TMPDIR:-/tmp}/cpaper-cloud-check.XXXXXX")"
LOG="$ROOT/verification/cloud/run-$(date -u +%Y%m%d-%H%M%S)-$$"
mkdir -p "$LOG"
export CPAPER_DATA_DIR="$TMP/data" STUDY_DATA_DIR="$TMP/study-data"
export CARGO_NET_OFFLINE=true npm_config_offline=true
trap 'rm -rf "$TMP"' EXIT
run() {
 local name="$1"; shift
 printf '\nCOMMAND: '; printf '%q ' "$@"; printf '\n'
 local result=0
 "$@" >"$LOG/$name.log" 2>&1 || result=$?
 cat "$LOG/$name.log"
 printf 'EXIT: %s\n' "$result" | tee -a "$LOG/$name.log"
 [[ "$result" == 0 ]] || return "$result"
}
run core-format "$CARGO" fmt --manifest-path "$ROOT/core/Cargo.toml" --check
run core-tests "$CARGO" test --locked --offline --manifest-path "$ROOT/core/Cargo.toml"
run core-build "$CARGO" build --locked --offline --manifest-path "$ROOT/core/Cargo.toml"
run web-tests npm --prefix "$ROOT/web" test
run web-build npm --prefix "$ROOT/web" run build
run shell-syntax bash -n "$ROOT/scripts/run-local.sh" "$ROOT/scripts/desktop-local.sh" "$ROOT/scripts/assemble-desktop.sh" "$ROOT/scripts/verify-local.sh" "$ROOT/scripts/verify-cloud.sh"
echo 'PASS: Linux core/web offline checks only. NOT RUN: macOS/Tauri build, native picker/UI, live providers, production Study.' | tee "$LOG/SUMMARY.txt"
echo "Logs: $LOG"
