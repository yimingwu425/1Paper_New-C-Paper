#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REFERENCE="/Users/yimingwu/Documents/Codex/2026-11-09/task/local-v5/study-app-v2"
for port in 47831 47832; do
 if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
   echo "端口 $port 已有进程占用。本脚本不会停止它；请确认已有 C-Paper 服务或退出后重试。" >&2; exit 1
 fi
done
if command -v cargo >/dev/null; then CARGO="$(command -v cargo)"
elif [[ -x "$REFERENCE/.toolchain/cargo/bin/cargo" ]]; then
 export CARGO_HOME="$REFERENCE/.toolchain/cargo" RUSTUP_HOME="$REFERENCE/.toolchain/rustup"
 CARGO="$CARGO_HOME/bin/cargo"
else echo '请先安装官方 Rust 工具链：https://rustup.rs' >&2; exit 1; fi
if [[ ! -d "$ROOT/web/node_modules" ]]; then npm --prefix "$ROOT/web" ci; fi
npm --prefix "$ROOT/web" run build
"$CARGO" build --manifest-path "$ROOT/core/Cargo.toml"
cd "$ROOT/core"
"$ROOT/core/target/debug/cpaper-core" & CORE_PID=$!
cleanup(){ kill "$CORE_PID" 2>/dev/null || true; [[ -z "${WEB_PID:-}" ]] || kill "$WEB_PID" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
npm --prefix "$ROOT/web" run dev & WEB_PID=$!
echo 'C-Paper 本地预览：http://127.0.0.1:47832 ；按 Ctrl+C 退出本脚本启动的服务。'
wait "$WEB_PID"
