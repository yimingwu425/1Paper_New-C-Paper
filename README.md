# C-Paper Rust

Private Rust + React continuation of C-Paper. The original Swift repository remains unchanged: https://github.com/yimingwu425/C-Paper

## Verified source baseline

This repository starts from the latest local Rust rewrite captured on 2026-11-10, not the older public GitHub source. The transfer archive and all 58 included files were SHA-256 verified before cloud development. Private downloads, user history, credentials, screenshots and build caches are excluded here.

Current baseline includes left navigation, cross-year/multi-season Paper 1–6 batch preview, a persistent cancellable/retryable download queue, destination selection, explicit legacy-data import and Study handoff receipts. The local baseline passed 44 Rust and 42 frontend tests on macOS. Those are historical baseline results, not a claim that new cloud changes or native macOS UI have been verified on Linux.

## Layout

- `next-rust/core`: Rust local API and persistence.
- `next-rust/web`: React frontend and tests.
- `next-rust/desktop`: Tauri macOS shell.
- `next-rust/scripts`: local build/run/check scripts.

Cloud continuation is restoring source selection/fallback and shared download concurrency/request pacing. Work stays in a separate branch until reviewed. No deployment or automatic modification of the Mac is performed.

## Checks

Use the committed lockfiles. Core: `cargo test --locked --manifest-path next-rust/core/Cargo.toml`. Web: `npm --prefix next-rust/web ci`, then `npm --prefix next-rust/web test` and `npm --prefix next-rust/web run build`. Native desktop checks require the macOS dependencies and a separate Mac validation pass.
