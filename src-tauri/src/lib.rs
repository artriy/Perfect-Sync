mod commands;
mod console_monitor;
mod managed_instance;
mod settings;
mod storage;

use tauri::Manager;
use tauri_plugin_log::{RotationStrategy, Target, TargetKind};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }));
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let log_builder = tauri_plugin_log::Builder::new()
        .level(if cfg!(debug_assertions) {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        })
        .filter(|metadata| metadata.target().starts_with("app_lib"))
        .max_file_size(5 * 1024 * 1024)
        .rotation_strategy(RotationStrategy::KeepOne)
        .clear_targets()
        .target(Target::new(TargetKind::LogDir {
            file_name: Some("perfect-sync".to_string()),
        }));
    #[cfg(debug_assertions)]
    let log_builder = log_builder.target(Target::new(TargetKind::Stdout));
    builder = builder.plugin(log_builder.build());
    builder = builder.plugin(tauri_plugin_process::init());
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            let data_dir = app.path().data_dir()?.join("Perfect-Sync");
            settings::initialize_app_data_dir(data_dir)?;
            let managed_data_dir = app.path().local_data_dir()?.join("Perfect-Sync");
            settings::initialize_managed_data_dir(managed_data_dir)?;
            let saved = settings::load()?;
            if let Some(storage_path) = saved.storage_path.as_deref() {
                let game_sources = saved
                    .game_instances
                    .iter()
                    .map(|instance| instance.path.clone().into())
                    .collect::<Vec<_>>();
                match storage::validate_configured_root(
                    storage_path,
                    &settings::default_managed_data_dir(),
                    &settings::app_data_dir(),
                    &game_sources,
                ) {
                    Ok(storage_root) => {
                        if let Err(error) = settings::set_managed_data_dir(storage_root) {
                            log::error!(
                                "configured managed storage is unavailable; using the local default: {error}"
                            );
                        }
                    }
                    Err(error) => log::error!(
                        "configured managed storage is unavailable; using the local default: {error}"
                    ),
                }
            }
            settings::reset_v016_profiles_once(&saved)?;
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(error) = app.deep_link().register_all() {
                    log::error!("deep-link protocol registration failed");
                    return Err(error.into());
                }
                log::info!("deep-link protocol registration completed");
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::preview_code,
            commands::detect_games,
            commands::inspect_game,
            commands::get_settings,
            commands::save_settings,
            commands::select_active_profile,
            commands::move_storage,
            commands::export_error_log,
            commands::game_running,
            commands::stop_game,
            commands::get_catalog,
            commands::refresh_catalog,
            commands::add_catalog_mod,
            commands::remove_catalog_mod,
            commands::reorder_catalog,
            commands::list_unmanaged_plugins,
            commands::quarantine_unmanaged_plugins,
            commands::delete_unmanaged_plugins,
            commands::import_unmanaged_plugins,
            commands::ensure_loader,
            commands::reinstall_loader,
            commands::loader_status,
            commands::collect_diagnostics,
            commands::export_support_bundle,
            commands::backup_save_data,
            commands::list_save_backups,
            commands::restore_save_data,
            commands::list_profiles,
            commands::save_profile,
            commands::delete_profile,
            commands::encode_lobby_code,
            commands::list_releases,
            commands::list_install_options,
            commands::list_tou_setup_options,
            commands::install_assets,
            commands::install_local_mod,
            commands::search_levelimposter_maps,
            commands::fetch_levelimposter_banner,
            commands::list_levelimposter_maps,
            commands::install_levelimposter_maps,
            commands::remove_levelimposter_maps,
            commands::install_asset,
            commands::add_mod,
            commands::set_mod_enabled,
            commands::set_mod_version,
            commands::remove_mod,
            commands::apply_lobby_code,
            commands::launch_profile,
            commands::launch_vanilla,
            commands::check_mod_updates,
            commands::apply_mod_updates,
            commands::sync_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
