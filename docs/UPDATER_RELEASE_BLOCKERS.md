# AeroTune updater release blocker record

**Status: updates remain disabled.** This file separates work the repository can implement from facts that must come from a real release operation or privileged Windows acceptance. A code change must never mark an external acceptance result as true.

## Evidence checked for the current workspace

Checked 2026-10-29 (Asia/Shanghai):

- No `AEROTUNE_UPDATER_CONFIG`, `AEROTUNE_UPDATE_ENDPOINT`, `AEROTUNE_UPDATE_PUBLIC_KEY`, `TAURI_SIGNING_PRIVATE_KEY`, or `TAURI_SIGNING_PRIVATE_KEY_PATH` environment inputs were present. Secret values are not printed or copied.
- Existing installer and executable samples (including `C:\Program Files\AeroTune\AeroTune.exe`) are Authenticode `NotSigned`; local installer sizes observed were approximately 2.69–2.73 MB. These old, unsigned samples do **not** establish a Tauri-signed updater ZIP size or safe transport/expanded limits.
- The exact scheduled task `AeroTune-SignedUpdater` is not registered.
- Current shell is not elevated. Hyper-V `Get-VM` returns access denied, so guest state/snapshots and update/rollback acceptance cannot be inspected here.
- The workspace has no `.git` metadata or configured remote; this session cannot publish artifacts or configure a protected CI secret/feed.

## Repository implementation status

The code already has useful pieces: bounded manifest parsing in the pinned vendored plugin, bounded artifact streaming and archive validation, signature/version binding, a journal schema and failure-injected atomic writer, a protected-store read-only startup gate, and task-definition/DACL checks. These are prototypes and unit-tested components, not a working installer transaction.

The critical missing code is an independent protected recovery host/supervisor outside `Program Files\AeroTune`, verified protected staging, a real install/rollback executor, journal integration across process crashes, and independently verified installed manifests. The current executable is not a safe supervisor for replacing its own installation tree. Therefore do not turn on `UPDATE_PIPELINE_READY`, DNS/proxy/download/task-attestation readiness, `RECOVERY_STORE_REQUIRED`, task registration, or bundle updater artifacts as a shortcut.

## Required real acceptance evidence

1. Create and back up a Tauri updater keypair in a reviewed secret manager; publish the public key only through reviewed local release configuration. Review secret ACL/provenance without exposing key content.
2. Operate one stable public HTTPS feed and retain the current and rollback-capable prior signed artifact.
3. Measure actual signed artifact and manifest sizes, review the manifest cap and byte/time limits, and record release hashes. Unsigned historic NSIS installers are not representative. Artifact cap evidence is not a signature or VM acceptance.
4. Complete/review the connection-time destination policy and system-proxy behavior, including redirects, DNS rebinding/private-address answers, proxy failure, and no direct fallback.
5. Implement a separately protected updater host, ProgramData provisioning, crash journal integration, install-result verification, terminal cleanup, and external recovery/rollback. Keep standard-user/UAC semantics explicit.
6. On disposable Windows VMs, test valid and invalid signed updates, interruption at every transaction phase, restoration/rollback, task definition/DACL, install/uninstall, and supported account scenarios. Preserve VM snapshot IDs, hashes/manifests, task ACL evidence, exit codes, and logs.
7. Only after independent review of the above evidence, have an authorized release owner explicitly approve gate changes and protect the signing secret in CI.

Until these prerequisites have actual evidence, release preflight and runtime gates must fail closed. Local tests and successful compilation are not substitutes for these items.
