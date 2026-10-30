# Local `tauri-plugin-updater` patch

- Upstream: crates.io `tauri-plugin-updater` 2.14.0; upstream license files are retained alongside the vendored source.
- Functional delta: bound updater manifest JSON reads to 256 KiB. `Content-Length` is rejected early when over the cap; streamed chunks are counted when length is absent or dishonest; truncated known-length bodies and invalid JSON fail closed.
- The cap is a prototype, not a release-approved value. AeroTune's `UPDATE_FEED_RESPONSE_BOUNDED_READY` and release-preflight gate remain false pending review and feed-format measurement.
- When upgrading the upstream crate, reapply/review this patch and rerun the package's bounded-manifest tests before changing the gate.
