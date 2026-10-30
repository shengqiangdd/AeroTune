# Windows install, upgrade, and uninstall acceptance

**Status: accepted for the scoped 1.5.9 smoke test.** The built executable ran on the host under standard privileges without installation, and the NSIS installer, app launch, and uninstall were exercised in the available Windows 10 VM after WebView2 was present. Dashboard/Process Management rendering and default retention of `%APPDATA%\AeroTune` were observed. This pass does not claim a blank offline machine can install without WebView2 or that the separate 1.5.8 upgrade path was verified.

## Observed diagnostic VM evidence — 2026-10-27

- **Host UI smoke (no installation):** The built 1.5.9 executable was launched directly on the host's Windows 11 build `26300` under standard privileges, without running the installer. The main window, Dashboard, and Process Management rendered; the badge showed `标准权限`. No process action was invoked. This verifies the non-installed UI path, not host installation behavior.
- **1.5.9 clean install and launch:** On the offline `AeroTune-Input-Win10-Admin-Control` diagnostic VM (trimmed Windows 10 build `19045.4170`, Secure Boot Off, built-in Administrator account), the first installer run failed because WebView2 was absent and the guest had no network. Installing Microsoft's signed x64 Evergreen Standalone WebView2 runtime separately allowed the 1.5.9 NSIS installer to complete into `C:\Program Files\AeroTune`. Launch from the created desktop shortcut rendered the Dashboard and Process Management pages. The title bar showed `管理员`; this is an admin-account test, not a standard-user identity test. No process action was invoked.
- **Admin-mode uninstall and data retention:** The uninstaller's `删除应用程序数据` checkbox was unchecked by default and left unchecked. After confirming termination of the running app, uninstall completed, the `Program Files\AeroTune` directory was gone, and `C:\Users\Administrator\AppData\Roaming\AeroTune` remained with its logs and JSON state files. This is empirical evidence for the unchecked-data-removal path, but not the standard-user/UAC path or a byte-for-byte sentinel manifest.
- **Separate upgrade follow-up:** The older 1.5.8 installer displayed a successful-completion page, but the installed executable/path was not verified and a 1.5.8-to-1.5.9 in-place upgrade was not completed. The diagnostic VM's final install state after this attempt is uncertain; do not treat it as clean or 1.5.8 as absent.
- **Unresolved offline prerequisite:** With no network and no existing WebView2 runtime, the current 1.5.9 installer failed. No release-config change was made. Decide whether offline deployment is required before considering Tauri's offline runtime bundle (about 205 MiB for the standalone package used here); otherwise document the online WebView2 bootstrapper prerequisite.
- **Not covered by this scoped pass:** Standard-user/UAC behavior, a 1.5.8-to-1.5.9 upgrade, UAC cancellation/interruption, explicit data cleanup, and accessibility/recovery scenarios. These are follow-up coverage for broader distribution, not prerequisites for the install/launch/uninstall smoke pass above.

## Intended data policy (pending broader VM confirmation)

- Keep the machine-wide install under protected `Program Files` and require normal Windows UAC for install, upgrade, and uninstall; never use quiet mode to evade elevation.
- Keep user state under `%APPDATA%\AeroTune` separate from the install tree.
- Preferred uninstall behavior is to remove only AeroTune's installed files, shortcuts, uninstaller metadata, and any AeroTune-owned scheduled tasks, while preserving per-user settings, history, journal, and recovery snapshots by default. The diagnostic 1.5.9 admin-mode uninstall left the data-removal checkbox unchecked by default and the `%APPDATA%\AeroTune` folder present; standard-user behavior and a byte-for-byte sentinel check remain unverified.
- Provide a separate, explicit manual cleanup instruction after uninstall for users who want to remove retained data. Do not remove arbitrary files or unrelated registry state. There is no supported in-app “delete all local data” flow yet.

## Additional disposable-VM coverage for broader distribution

Use snapshots and a test account; record Windows edition/build, account privilege, installer SHA-256, and before/after file/registry/task manifests. Never run these destructive acceptance steps on a production machine.

| Scenario | Procedure | Required result / evidence |
| --- | --- | --- |
| Standard-user clean install | Launch the NSIS installer as a standard user and complete the genuine Windows UAC consent/credential prompt. | No silent elevation; install is machine-wide under the fixed AeroTune Program Files directory; standard user cannot write the install tree. Capture UAC result and ACLs. |
| Upgrade with existing state | Install the prior supported release, launch AeroTune to create representative config, journal, history, logs, startup-delay backup, and a sentinel file under `%APPDATA%\AeroTune`; record hashes; install the new release in place. | App files update in place; the sentinel and supported user state remain byte-for-byte or migrate through an explicit tested schema; no state is silently reset. Record manifests and app startup result. |
| Standard-user uninstall | Close AeroTune, start uninstall as a standard user, complete any normal UAC prompt, then inspect the install tree, shortcuts, uninstall registry entries, tasks, and profile data. | Only documented AeroTune-owned machine artifacts are removed. The preferred policy preserves `%APPDATA%\AeroTune`; confirm this empirically and check unrelated profile data is untouched. |
| Retained-data cleanup | After uninstall, follow the proposed manual cleanup instruction in the test account. | Only the exact AeroTune data directory is removed after backup/confirmation; adjacent AppData content remains. Record before/after manifests. |
| Reinstall after uninstall | Reinstall without deleting retained user state, then repeat with state explicitly removed. | Preserved state is handled predictably and safely; clean-state behavior is distinct and documented. |
| UAC denial / interruption | Cancel the UAC prompt and separately interrupt a disposable install/upgrade at safe checkpoints. | Cancellation does not partially remove the old install or alter user data; interrupted states recover safely or expose clear diagnostics. |

For a broader GA review, run this matrix on each supported Windows client version. Record task names and ownership; a task not created by the product must never be removed. Review generated installer/uninstaller artifacts rather than inferring behavior solely from `tauri.conf.json`.

## Current controls and limits

`src-tauri/tauri.conf.json` selects NSIS and per-machine mode; `src-tauri/build.rs` and the build/release contracts reject custom install paths, templates, and hooks that could bypass the reviewed boundary. The app's profile data path is implemented in `src-tauri/src/util.rs`. The smoke evidence above establishes the tested admin-mode install/uninstall path and default data retention. Do not extrapolate it to the separate coverage items listed above; broader GA readiness is tracked separately in `RELEASE_READINESS.md`.
