#!/usr/bin/env bash
# Offline deterministic checks. No app launch, external providers or production data.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "${1:-}" in
 --help) echo 'Usage: verify-local.sh [--help]. Offline Rust/web/desktop tests and builds; see README for separate live/GUI acceptance.'; exit 0;;
 "") ;;
 *) echo 'Unknown option. Live integration is deliberately separate.' >&2; exit 2;;
esac
REFERENCE="/Users/yimingwu/Documents/Codex/2026-11-09/task/local-v5/study-app-v2"
if command -v cargo >/dev/null; then CARGO="$(command -v cargo)"
elif [[ -x "$REFERENCE/.toolchain/cargo/bin/cargo" ]]; then
 export CARGO_HOME="$REFERENCE/.toolchain/cargo" RUSTUP_HOME="$REFERENCE/.toolchain/rustup"
 CARGO="$CARGO_HOME/bin/cargo"
else echo 'Rust toolchain missing; no automatic install performed.' >&2; exit 1; fi
[[ -d "$ROOT/web/node_modules" ]] || { echo 'Web dependencies missing; install separately using the lockfile.' >&2; exit 1; }
TMP="$(mktemp -d "${TMPDIR:-/tmp}/cpaper-check.XXXXXX")"
LOG="$ROOT/verification/local-$(date +%Y%m%d-%H%M%S)-$$"
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
run core-tests "$CARGO" test --locked --offline --manifest-path "$ROOT/core/Cargo.toml"
run core-build "$CARGO" build --locked --offline --manifest-path "$ROOT/core/Cargo.toml"
run web-tests npm --prefix "$ROOT/web" test
run web-build npm --prefix "$ROOT/web" run build
run desktop-check "$CARGO" check --locked --offline --manifest-path "$ROOT/desktop/Cargo.toml"
run desktop-build "$CARGO" build --locked --offline --manifest-path "$ROOT/desktop/Cargo.toml"
run shell-syntax bash -n "$ROOT/scripts/run-local.sh" "$ROOT/scripts/desktop-local.sh" "$ROOT/scripts/assemble-desktop.sh" "$ROOT/scripts/verify-local.sh"
echo 'PASS: offline local checks; live providers, isolated Study integration and native GUI not executed.' | tee "$LOG/SUMMARY.txt"
echo "Logs: $LOG"
