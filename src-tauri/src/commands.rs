use perfect_sync_core::catalog::{parse, Catalog};
use perfect_sync_core::preview::{preview, Preview};

const CATALOG_JSON: &str = include_str!("../../core/fixtures/catalog.sample.json");

fn catalog() -> Catalog {
    parse(CATALOG_JSON).expect("bundled catalog parses")
}

/// Decode a PERFECT- code into a UI preview (diff vs the installed set).
#[tauri::command]
pub fn preview_code(code: String, installed: Vec<(String, String)>) -> Result<Preview, String> {
    preview(&code, &catalog(), &installed).map_err(|e| e.to_string())
}
