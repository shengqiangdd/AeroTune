#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

const LEGACY_UPDATE_ONLY_ARGUMENT: &str = "--aerotune-update-only";

fn main() {
    if std::env::args_os()
        .skip(1)
        .any(|argument| argument == LEGACY_UPDATE_ONLY_ARGUMENT)
    {
        // Reject any stale scheduled-task invocation; never fall through to launching the UI elevated.
        std::process::exit(2);
    }
    let _update_gate = match aerotune_lib::try_acquire_app_update_gate() {
        Ok(gate) => gate,
        Err(_) => std::process::exit(2),
    };
    aerotune_lib::run()
}
