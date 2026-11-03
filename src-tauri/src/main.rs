#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::{Duration, Instant};

const LEGACY_UPDATE_ONLY_ARGUMENT: &str = "--aerotune-update-only";
/// Passed only by `relaunch_as_admin` (admin.rs). The elevated replacement is started while the old
/// process still owns the launch gate, so it must wait for that owner to exit instead of failing at once.
const RELAUNCH_ELEVATED_ARGUMENT: &str = "--aerotune-relaunch-elevated";
const RELAUNCH_GATE_WAIT: Duration = Duration::from_secs(20);
const RELAUNCH_GATE_POLL: Duration = Duration::from_millis(100);
/// Must match `app.windows[0].title` in tauri.conf.json (checked by a contract test).
#[cfg(windows)]
const MAIN_WINDOW_TITLE: &str = "AeroTune · 系统优化中枢";

fn main() {
    if std::env::args_os()
        .skip(1)
        .any(|argument| argument == LEGACY_UPDATE_ONLY_ARGUMENT)
    {
        // Reject any stale scheduled-task invocation; never fall through to launching the UI elevated.
        std::process::exit(2);
    }
    let relaunched = std::env::args_os()
        .skip(1)
        .any(|argument| argument == RELAUNCH_ELEVATED_ARGUMENT);
    let _update_gate = match acquire_launch_gate(relaunched) {
        Ok(gate) => gate,
        Err(_) => {
            // A refused ordinary launch means another instance (often hidden in the tray) is running:
            // bring it forward instead of exiting silently so the user's click is not lost.
            if !relaunched {
                focus_running_instance();
            }
            std::process::exit(2)
        }
    };
    aerotune_lib::run()
}

/// Ordinary launches fail closed immediately. A relaunch after elevation retries for a bounded time:
/// the previous process exits only after its UAC hand-off, and its abandoned gate is recreated once
/// the last handle to it closes.
fn acquire_launch_gate(
    wait_for_previous_instance: bool,
) -> Result<aerotune_lib::UpdateGateGuard, String> {
    let deadline = Instant::now() + RELAUNCH_GATE_WAIT;
    loop {
        match aerotune_lib::try_acquire_app_update_gate() {
            Ok(gate) => return Ok(gate),
            Err(error) if !wait_for_previous_instance || Instant::now() >= deadline => {
                return Err(error)
            }
            Err(_) => std::thread::sleep(RELAUNCH_GATE_POLL),
        }
    }
}

#[cfg(windows)]
fn focus_running_instance() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        FindWindowW, IsIconic, SetForegroundWindow, ShowWindow, SW_RESTORE, SW_SHOW,
    };

    let title: Vec<u16> = MAIN_WINDOW_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // Best effort only: this never starts, stops or reconfigures anything in the running instance.
    unsafe {
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        if window.is_null() {
            return;
        }
        let command = if IsIconic(window) != 0 {
            SW_RESTORE
        } else {
            SW_SHOW
        };
        ShowWindow(window, command);
        SetForegroundWindow(window);
    }
}

#[cfg(not(windows))]
fn focus_running_instance() {}
