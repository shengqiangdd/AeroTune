#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args_os()
        .skip(1)
        .any(|argument| argument == aerotune_lib::UPDATE_ONLY_ARGUMENT)
    {
        std::process::exit(aerotune_lib::run_update_only());
    }
    let _update_gate = match aerotune_lib::try_acquire_app_update_gate() {
        Ok(gate) => gate,
        Err(_) => std::process::exit(2),
    };
    aerotune_lib::run()
}
