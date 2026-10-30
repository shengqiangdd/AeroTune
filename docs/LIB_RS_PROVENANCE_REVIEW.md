# `src-tauri/src/lib.rs` provenance review

**Disposition: unresolved release gate.** This review records what can be verified from the current workspace; it does not establish that the reconstructed source matches an authoritative revision.

## Provenance evidence

- `git rev-parse --show-toplevel` fails in the workspace (`fatal: not a git repository`), so commit history, blame, and an authoritative diff are unavailable.
- Workspace searches found no `*lib*.bak`, `*lib*.old`, or `lib.rs.*` backup files.
- The workspace does contain the older [1.5.0 patch2 source ZIP](../releases/2026-10-11-patch2/AeroTune_1.5.0_2026-10-11-patch2-source.zip); its extracted `src-tauri/src/lib.rs` was 9479 bytes with SHA-256 `990155833230E26B6794B7E083B104BD71D112741EB5198F6829FA4EAE85C78C`. It matches neither the 16744-byte checkpoint nor the current 17061-byte file, so it does not resolve the current-source provenance.
- A prior workspace note says the file was reconstructed from captured fragments after accidental overwrite; no complete pre-overwrite source or trusted backup was found.
- The user reports on 2026-10-25 that the project was developed collaboratively and that `lib.rs` had always been intact. This is an authorship/intent assertion; it does not by itself explain the 317-byte difference. The current workspace is the audit target, and the prior conflicting note remains unverified.
- Previously recorded local checkpoint: **SHA-256 `271EE048F763546225BE2989BA9D285CC992037DE45FAF669A75A0A73D983CE7`**, 17061 bytes. This entry was updated on 2026-10-25 after full source/permission review, independent review, presentation of findings, and explicit user approval. The former checkpoint before approval was SHA-256 `2B3A31A3DA9815BDA72CF65DB4B01FB6BB7F525C7BA8D5F3FAC5F86437656971`, 16744 bytes. A size/hash mismatch alone does not prove corruption or unauthorized modification. This current checkpoint records the reviewed local bytes; it is not proof of origin, authorship, or intended behavior. Do not replace it to bypass the gate; any future change still requires complete source/permission review, independent review, presentation of findings, and explicit user confirmation.

## Current-structure review

- The crate exposes the update-only argument, update gate guard, and update-only entrypoint separately from the ordinary Tauri app.
- `run()` builds the ordinary Tauri application with explicit plugins and managed state. Its `invoke_handler` uses an explicit `generate_handler!` allowlist.
- After the explicitly approved checkpoint update, [`check-tauri-command-registration.mjs`](../scripts/check-tauri-command-registration.mjs) passes: **126** distinct frontend IPC command names match the explicit Rust handler allowlist, and the ordinary app has no updater initialization. This is a name-set check, not a security review of every handler or proof that the allowlist is historically complete.
- The inspected setup wires tray actions to the shared mode application, starts statistics after setup, supports the explicit `--silent` hide path, and hides on close only when close-to-tray is configured. Exit handling attempts to restore process scheduling and foreground priority snapshots.
- Daily log pruning is limited to regular files with the AeroTune date-suffixed log name and a 14-day age threshold.
- This source-level review plus passing compilation/contracts can detect some structural regressions, but cannot detect behavior omitted during reconstruction or verify all runtime paths on supported Windows versions.

## Required acceptance before merge or release

1. Obtain a trusted source revision or backup with verifiable origin (for example, a signed source archive or approved repository commit); do not treat the fingerprint above as that authority.
2. Compare the complete file, not just command names. Independently review module declarations/exports, update-only separation, the entire IPC allowlist, plugin/state setup, tray and silent-start behavior, close/exit handling, and logging/retention.
3. Re-run the command-registration and release-contract suites; review authorization in the corresponding Rust handlers and exercise startup, tray, UAC, shutdown, and recovery behavior in supported disposable Windows VMs.
4. Record the authoritative commit/archive digest, full diff, reviewer(s), findings, and sign-off; update the local fingerprint only after that review.
5. Keep release readiness blocked until this evidence exists. Keep the updater disabled under its separate signed-feed, protected-staging/rollback, and Windows-acceptance gates.
