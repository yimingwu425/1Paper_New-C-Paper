# Cloud verification: 2026-11-10

## Preservation and privacy

The source-only baseline is commit `58ca512c6f05e94b8b7d100215d852aa00390c11` in
the private `yimingwu425/C-Paper-Rust` repository. This continuation adds the 29
source files listed in `PRESERVATION_MANIFEST.json`, plus documentation and CI.
The original cloud working tree remains intact. No user database, downloaded
paper, account credential, private runtime log, screenshot, cache, or Swift
source is included. The EasyPaper protocol constants were checked against the
existing Swift adapter; they are protocol constants, not user credentials.

## Re-run results

- `bash next-rust/scripts/verify-cloud.sh`: passed on Linux.
- Rust tests: 68 passed, zero failures.
- React/component tests: 65 passed, zero failures.
- Rust build, Rust formatting, TypeScript check, Vite production build, and shell syntax: passed.
- Isolated real-core HTTP smoke: passed health, settings persistence and rejected-invalid-setting atomicity, frozen source/rate/concurrency, verified-file skip, and idempotent start replay. It used a temporary data root and a synthetic PDF, with no production Study service or upstream request.
- Existing source diff whitespace check: passed.

These results are from the preserved source. CI should be checked for the exact
published commit; this document does not assume a future CI result.

## Implemented behavior

- Auto mode and four manual sources: FrankCIE, EasyPaper, PastPapers, PapaCambridge.
- Manual mode never silently falls back. Attempts distinguish successful, empty, and failed responses.
- Frozen batch/queue controls: concurrency defaults to 4 (range 1–16), request rate defaults to 5 requests/second (range 1–20). This is not bandwidth throttling.
- Shared request admission and 429 cooldown; a failed item is reported honestly and is not silently retried or redirected to another source.
- Source-bound identities, hash-verified cache reuse, no-overwrite persistence, cancellation, and unresolved Study-submission safeguards.
- Settings saves remain serialized; stale search previews are invalidated after a source change.

## Not accepted as complete

- Prior bounded live canaries attempted one 9709/2023/June query per source. All four produced transport failures in that cloud runtime, and no PDF canary ran. This does not establish that the upstream sources are down and is not a live-source pass. No new live requests were made during preservation.
- PapaCambridge currently has only the audited 9709 mapping and no subject-catalog adapter.
- Application-managed proxy configuration is not implemented; the inherited HTTP client uses `no_proxy`.
- Browser QA did not load the application: a prior headless launch was blocked by the runtime, and the supported cloud browser could not connect to the isolated local server. Component tests are not visual or end-to-end browser acceptance. No security/network restriction was changed or bypassed during preservation.
- macOS/Tauri build, real native directory-picker interactions, signing, notarization, and packaging were not performed on Linux.
- No merge, deployment, production Study write, or automatic update to the user's Mac was performed.

## Reproduce on Linux

Install the official Rust toolchain and Node 24, then install locked dependencies:

```sh
npm --prefix next-rust/web ci
cargo fetch --locked --manifest-path next-rust/core/Cargo.toml
bash next-rust/scripts/verify-cloud.sh
```

The verification script performs its tests offline with temporary data roots. It
does not install dependencies, contact real providers, or exercise macOS UI.
