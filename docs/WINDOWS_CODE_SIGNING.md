# Windows signing policy

## Current status

No current AeroTune executable or NSIS installer has an Authenticode signature. The selected release path is the standard Tauri updater plus GitHub Releases; no SignPath application or Authenticode certificate is configured.

## Tauri updater Minisign

The Tauri updater requires a Minisign keypair. The public key is embedded in the app/release overlay and the private key signs the updater archive. The release design stores the private key only in the protected GitHub Environment `aerotune-release` as `TAURI_SIGNING_PRIVATE_KEY` (and, if encrypted, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`). The public key is a non-secret repository variable named `AEROTUNE_UPDATE_PUBLIC_KEY`.

Generate the pair once on a trusted machine outside the repository. Protect the GitHub environment with required reviewers and branch restrictions; keep an offline backup and document rotation/recovery. Never commit, paste, print, or expose the private key to frontend builds. The build wrapper strips signing variables from frontend/Cargo checks and supplies them only to the Tauri bundle stage.

A Minisign signature authenticates updater payload bytes and the signed version. It does **not** sign the PE installer with Authenticode, establish a Windows publisher identity, or guarantee SmartScreen reputation. Windows UAC may show an Unknown publisher prompt even when the Tauri updater signature verifies correctly.

## Optional separate Authenticode process

If a named publisher identity or better SmartScreen reputation is later required, that needs a separate Authenticode certificate/signing workflow with its own credentials, timestamping, publisher verification, and approval policy. It must not replace or reuse the Tauri updater Minisign key. No such process is currently configured or claimed.
