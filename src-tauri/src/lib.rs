mod commands;
mod settings;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .setup(|app| {
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
      commands::ensure_loader,
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
      commands::launch_profile
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
