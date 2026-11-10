#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REFERENCE="/Users/yimingwu/Documents/Codex/2026-11-09/task/local-v5/study-app-v2"
if lsof -nP -iTCP:47831 -sTCP:LISTEN >/dev/null 2>&1; then
 echo '47831 已有服务。请先确认并退出已有 C-Paper 核心；本脚本不终止已有进程。' >&2; exit 1
fi
if command -v cargo >/dev/null; then CARGO="$(command -v cargo)"
elif [[ -x "$REFERENCE/.toolchain/cargo/bin/cargo" ]]; then
 export CARGO_HOME="$REFERENCE/.toolchain/cargo" RUSTUP_HOME="$REFERENCE/.toolchain/rustup"
 CARGO="$CARGO_HOME/bin/cargo"
else echo '请安装官方 Rust 工具链：https://rustup.rs' >&2; exit 1; fi
[[ -d "$ROOT/web/node_modules" ]] || npm --prefix "$ROOT/web" ci
npm --prefix "$ROOT/web" run build
"$CARGO" build --manifest-path "$ROOT/core/Cargo.toml"
"$CARGO" build --manifest-path "$ROOT/desktop/Cargo.toml"
bash "$ROOT/scripts/assemble-desktop.sh"
export CPAPER_WEB_DIR="$ROOT/web/dist"
"$ROOT/core/target/debug/cpaper-core" & CORE_PID=$!
cleanup(){ kill "$CORE_PID" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
for attempt in {1..40}; do
 if curl -fsS http://127.0.0.1:47831/api/health >/dev/null; then break; fi
 kill -0 "$CORE_PID" 2>/dev/null || { echo '本地核心启动失败。' >&2; exit 1; }
 sleep .25
done
curl -fsS http://127.0.0.1:47831/api/health >/dev/null
"$ROOT/desktop/C-Paper Next.app/Contents/MacOS/cpaper-desktop"
