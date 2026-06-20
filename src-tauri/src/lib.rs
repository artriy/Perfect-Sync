mod commands;
mod settings;

use tauri::Manager;

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
  builder
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_deep_link::init())
    .setup(|app| {
      #[cfg(desktop)]
      {
        use tauri_plugin_deep_link::DeepLinkExt;
        let _ = app.deep_link().register("perfectsync");
      }
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      commands::preview_code,
      commands::detect_games,
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
      commands::install_asset,
      commands::add_mod,
      commands::set_mod_enabled,
      commands::set_mod_version,
      commands::remove_mod,
      commands::apply_lobby_code,
      commands::launch_profile,
      commands::check_update,
      commands::open_url
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
