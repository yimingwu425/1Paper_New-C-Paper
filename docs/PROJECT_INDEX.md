# Rust continuation project index

This private repository contains the Rust/React continuation only. The separate
Swift repository and user data are not part of this change.

- `next-rust/core/src/providers.rs`: explicit source selection, automatic fallback, and source-attempt records.
- `next-rust/core/src/provider_easypaper.rs`: EasyPaper directory protocol and fresh download URLs.
- `next-rust/core/src/provider_html.rs`: bounded PastPapers and PapaCambridge listing adapters.
- `next-rust/core/src/download_jobs.rs`: persisted queue, concurrency, request pacing, cancellation, and 429 cooldown.
- `next-rust/core/src/{preferences,batch_preview,storage,study_batch}.rs`: settings, frozen previews, source verification, and Study handoff protections.
- `next-rust/web/src/{SourceSettings,SourceAttempts}.tsx`: source/rate settings and attempt provenance.
- `next-rust/web/src/settings.ts`: serialized settings updates.
- `next-rust/scripts/verify-cloud.sh`: offline Linux core/frontend verification, after dependencies are installed.
- `docs/CLOUD_VERIFICATION.md`: verified results and remaining acceptance boundaries.
- `PRESERVATION_MANIFEST.json`: SHA256 inventory of the preserved 29 changed source files.

Native macOS/Tauri validation remains a separate step. A Linux pass is not a native release certification.
