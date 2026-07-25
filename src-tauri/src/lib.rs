mod commands;
mod settings;

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
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            let data_dir = app.path().data_dir()?.join("Perfect-Sync");
            settings::initialize_app_data_dir(data_dir)?;
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
            commands::game_running,
            commands::get_catalog,
            commands::refresh_catalog,
            commands::add_catalog_mod,
            commands::remove_catalog_mod,
            commands::reorder_catalog,
            commands::ensure_loader,
            commands::reinstall_loader,
            commands::loader_status,
            commands::list_profiles,
            commands::save_profile,
            commands::delete_profile,
            commands::encode_lobby_code,
            commands::list_releases,
            commands::list_install_options,
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
            commands::check_update,
            commands::open_url,
            commands::sync_profile
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
