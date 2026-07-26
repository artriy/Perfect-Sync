// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if app_lib::run_console_monitor_if_requested() {
        return;
    }
    app_lib::run();
}
