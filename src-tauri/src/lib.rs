mod admin;
mod auto;
mod brightness;
mod cleaner;
mod defrag;
mod diag;
mod disk_analyzer;
mod disk_trend;
mod experiment_insight;
mod gpu;
mod journal;
mod network;
mod power;
mod processes;
mod profile;
mod recommendations;
mod reliability;
mod residency;
mod rootcause;
mod scene_shadow;
mod services;
mod software_updates;
mod startup;
mod stats;
mod tasks;
mod tweaks;
mod updater;
mod util;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, Manager};

pub use updater::UpdateGateGuard;

pub fn try_acquire_app_update_gate() -> Result<UpdateGateGuard, String> {
    updater::try_acquire_app_update_gate()
}

/// non-blocking 日志 worker 必须存活到进程结束
static LOG_GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();
static CPU_SCHEDULER_CLEANUP_DONE: AtomicBool = AtomicBool::new(false);

pub(crate) fn cpu_scheduler_cleanup_requested() -> bool {
    CPU_SCHEDULER_CLEANUP_DONE.load(Ordering::SeqCst)
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.show().is_ok() {
            let _ = window.set_focus();
            let _ = app.emit("main-window-shown", ());
        }
    }
}

fn publish_tray_mode_feedback(app: &tauri::AppHandle, event: &str, title: &str, message: &str) {
    let visible = app
        .get_webview_window("main")
        .map(|window| window.is_visible().unwrap_or(false))
        .unwrap_or(false);
    if visible {
        let _ = app.emit(event, message.to_string());
        return;
    }

    use tauri_plugin_notification::NotificationExt;
    if app
        .notification()
        .builder()
        .title(title)
        .body(message.to_string())
        .show()
        .is_err()
    {
        show_main_window(app);
        let _ = app.emit(event, message.to_string());
    }
}

fn apply_tray_mode(app: &tauri::AppHandle, mode: &'static str) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || match crate::tweaks::apply_mode_core(mode) {
        Ok(actions) => {
            let message = actions.join("\n");
            tracing::info!(mode, "托盘场景模式已应用");
            publish_tray_mode_feedback(
                &app,
                "tray-mode-applied",
                "AeroTune 场景模式已应用",
                &message,
            );
        }
        Err(error) => {
            tracing::warn!(mode, %error, "托盘场景模式应用失败");
            let message = if crate::util::is_admin() {
                error
            } else {
                format!("{error}（当前为标准权限；如需管理员功能，请在设置页提权后重试）")
            };
            publish_tray_mode_feedback(
                &app,
                "tray-mode-failed",
                "AeroTune 场景模式未应用",
                &message,
            );
        }
    });
}

const LOG_RETENTION: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 60 * 60);

fn should_prune_daily_log(
    name: &std::ffi::OsStr,
    is_file: bool,
    modified: std::time::SystemTime,
    now: std::time::SystemTime,
) -> bool {
    if !is_file {
        return false;
    }
    let Some(date) = name
        .to_str()
        .and_then(|file_name| file_name.strip_prefix("aerotune.log."))
    else {
        return false;
    };
    if chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return false;
    }
    let Some(cutoff) = now.checked_sub(LOG_RETENTION) else {
        return false;
    };
    modified < cutoff
}

fn prune_old_logs(dir: &std::path::Path) {
    let now = std::time::SystemTime::now();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) else {
            continue;
        };
        if should_prune_daily_log(&entry.file_name(), file_type.is_file(), modified, now) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn init_logging() {
    if LOG_GUARD.get().is_some() {
        return;
    }
    let dir = crate::util::app_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    prune_old_logs(&dir);
    let writer = tracing_appender::rolling::daily(dir, "aerotune.log");
    let (writer, guard) = tracing_appender::non_blocking(writer);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .init();
    let _ = LOG_GUARD.set(guard);
    tracing::info!(
        "AeroTune v{} 启动 admin={}",
        env!("CARGO_PKG_VERSION"),
        crate::util::is_admin()
    );
}

pub fn run() {
    init_logging();
    let cache = Arc::new(stats::Cache::new());
    let diag = Arc::new(Mutex::new(diag::Engine::new()));
    let auto = Arc::new(Mutex::new(auto::AutoState::new()));
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(cache.clone())
        .manage(diag.clone())
        .manage(auto.clone())
        .manage(Mutex::new(HashSet::<u32>::new()))
        .invoke_handler(tauri::generate_handler![
            stats::get_stats,
            stats::get_diagnostics,
            recommendations::get_smart_recommendations,
            profile::get_process_insights,
            rootcause::get_anomaly_context,
            auto::set_autopilot,
            scene_shadow::get_scene_suggestions,
            scene_shadow::dismiss_scene_suggestion,
            residency::get_residency_suggestions,
            residency::dismiss_residency_suggestion,
            experiment_insight::analyze_experiment_consistency,
            disk_trend::get_disk_health_trends,
            disk_trend::dismiss_disk_trend,
            processes::list_processes,
            processes::kill_process,
            processes::set_priority,
            processes::suspend_process,
            processes::resume_process,
            processes::trim_memory,
            processes::smart_trim,
            processes::preview_process_memory_optimization,
            processes::optimize_process_memory,
            processes::set_process_affinity,
            power::list_power_plans,
            power::set_power_plan,
            power::battery_report,
            power::power_diagnostics,
            power::get_epp,
            power::set_epp,
            power::hibernate_info,
            power::set_hibernate,
            power::list_redundant_plans,
            power::remove_plan,
            tweaks::get_tweak_states,
            tweaks::set_tweak,
            tweaks::apply_mode,
            tweaks::get_mode_state,
            tweaks::list_gpu_prefs,
            tweaks::set_gpu_pref,
            tweaks::set_update_pause,
            tweaks::get_update_pause,
            tweaks::get_telemetry_minimal,
            tweaks::set_telemetry_minimal,
            auto::get_auto_settings,
            auto::set_auto_setting,
            auto::get_auto_state,
            auto::add_game_item,
            auto::remove_game_item,
            auto::set_protected_apps,
            auto::set_rule,
            auto::set_auto_num,
            auto::get_state_seconds,
            startup::list_startup,
            startup::toggle_startup,
            startup::delay_startup_item,
            startup::undelay_startup_item,
            startup::list_delayed_startup,
            startup::boot_history,
            startup::startup_impacts,
            reliability::get_reliability_events,
            services::list_services,
            services::set_service_startup,
            tasks::list_logon_tasks,
            tasks::set_task_state,
            disk_analyzer::scan_disk,
            disk_analyzer::cancel_disk_scan,
            disk_analyzer::pick_duplicate_scan_root,
            disk_analyzer::scan_duplicate_files,
            disk_analyzer::cancel_duplicate_scan,
            disk_analyzer::recycle_delete,
            disk_analyzer::disk_health,
            defrag::defrag_status,
            defrag::defrag_optimize,
            defrag::defrag_analyze,
            defrag::set_defrag_schedule,
            cleaner::list_appx,
            cleaner::remove_appx,
            cleaner::compactos_status,
            cleaner::set_compactos,
            cleaner::mem_compression_status,
            cleaner::set_mem_compression,
            cleaner::scan_residuals,
            cleaner::remove_residual,
            cleaner::dism_component_cleanup,
            cleaner::dism_component_cleanup_reset,
            cleaner::scan_junk,
            cleaner::preview_junk,
            cleaner::clean_junk,
            cleaner::empty_recycle_bin,
            network::list_wifi_profiles,
            network::list_adapters,
            network::get_dns,
            network::set_dns,
            network::reset_dns,
            network::flush_dns,
            network::get_tcp_autotuning,
            network::set_tcp_autotuning,
            network::ping_quality,
            network::list_nic_power,
            network::set_nic_power,
            network::list_tcp_latency,
            network::set_tcp_latency,
            admin::create_restore_point,
            admin::export_config,
            admin::save_text_export,
            admin::import_config,
            admin::list_shadow_storage,
            admin::delete_oldest_shadow,
            admin::set_shadow_max,
            admin::open_path,
            admin::open_settings_page,
            admin::is_admin,
            admin::is_silent_start,
            admin::relaunch_as_admin,
            admin::get_system_info,
            admin::export_diagnostic_report,
            admin::set_autostart,
            admin::get_autostart,
            journal::list_changes,
            journal::revert_change,
            journal::revert_change_group,
            journal::revert_all,
            journal::clear_journal,
            updater::get_signed_update_readiness,
            updater::check_signed_update,
            software_updates::check_winget_updates,
            updater::request_signed_update,
        ])
        .setup(move |app| {
            use tauri::{
                menu::{Menu, MenuItem, PredefinedMenuItem},
                Manager,
            };

            // ---- 系统托盘 ----
            let show_i = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let m_game = MenuItem::with_id(app, "m_game", "游戏模式", true, None::<&str>)?;
            let m_power = MenuItem::with_id(app, "m_power", "省电模式", true, None::<&str>)?;
            let m_dev = MenuItem::with_id(app, "m_dev", "编码模式", true, None::<&str>)?;
            let m_none = MenuItem::with_id(app, "m_none", "退出场景模式", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出 AeroTune", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[
                    &show_i, &sep1, &m_game, &m_power, &m_dev, &m_none, &sep2, &quit,
                ],
            )?;
            let _tray = tauri::tray::TrayIconBuilder::with_id("main-tray")
                .icon(
                    tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?,
                )
                .menu(&menu)
                .tooltip("AeroTune · 系统优化中枢")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "m_game" => apply_tray_mode(app, "game"),
                    "m_power" => apply_tray_mode(app, "power"),
                    "m_dev" => apply_tray_mode(app, "dev"),
                    "m_none" => apply_tray_mode(app, "none"),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;

            // ---- 静默启动（开机自启带 --silent，只驻留托盘）----
            if std::env::args().any(|argument| argument == "--silent") {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            stats::start(app.handle().clone(), cache, diag, auto);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if crate::auto::load_settings().close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. })
                && !CPU_SCHEDULER_CLEANUP_DONE.swap(true, Ordering::SeqCst)
            {
                let state_handle = app.state::<auto::AutoStateLock>();
                let mut state = state_handle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Err(error) = auto::restore_scheduler_snapshot_map(&mut state.scheduler_originals) {
                    tracing::error!(%error, "退出时还原后台进程 CPU 调度失败；记录将在下次启动时重试");
                }
                if let Some(priority) = state.foreground_priority_original.clone() {
                    if let Err(error) = auto::restore_priority_snapshot(&priority) {
                        tracing::error!(%error, "退出时还原前台进程优先级失败；记录将在下次启动时重试");
                    } else {
                        state.foreground_priority_original = None;
                        let _ = auto::persist_foreground_snapshot(None);
                    }
                }
                // 退出时恢复被变暗的屏幕亮度（同步执行，退出路径不介意耗时）。
                let saved_brightness = state.adaptive.brightness.saved;
                brightness::restore_on_exit(saved_brightness);
                state.adaptive.brightness.saved = None;
            }
        });
}

#[cfg(test)]
mod log_retention_tests {
    use super::{should_prune_daily_log, LOG_RETENTION};
    use std::ffi::OsStr;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn only_regular_old_aerotune_daily_logs_are_pruned() {
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000);
        let expired = now - LOG_RETENTION - Duration::from_secs(1);
        let exact_cutoff = now - LOG_RETENTION;
        let recent = now - LOG_RETENTION + Duration::from_secs(1);

        assert!(should_prune_daily_log(
            OsStr::new("aerotune.log.2026-09-01"),
            true,
            expired,
            now
        ));
        assert!(!should_prune_daily_log(
            OsStr::new("aerotune.log"),
            true,
            expired,
            now
        ));
        assert!(!should_prune_daily_log(
            OsStr::new("other.log.2026-09-01"),
            true,
            expired,
            now
        ));
        assert!(!should_prune_daily_log(
            OsStr::new("aerotune.log.backup"),
            true,
            expired,
            now
        ));
        assert!(!should_prune_daily_log(
            OsStr::new("aerotune.log.2026-99-99"),
            true,
            expired,
            now
        ));
        assert!(!should_prune_daily_log(
            OsStr::new("aerotune.log.2026-10-01"),
            false,
            expired,
            now
        ));
        assert!(!should_prune_daily_log(
            OsStr::new("aerotune.log.2026-10-06"),
            true,
            exact_cutoff,
            now
        ));
        assert!(!should_prune_daily_log(
            OsStr::new("aerotune.log.2026-10-07"),
            true,
            recent,
            now
        ));
    }
}
