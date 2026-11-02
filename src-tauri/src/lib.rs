mod commands;
mod db;
mod document_types;
mod documents;
mod error;
mod frontmatter;
mod github;
mod ids;
mod markdown;
mod models;
mod planboard;
mod posts;
mod screenshot;
mod search;
mod settings;
mod state;
mod stories;
mod sync;
mod syncer;
mod threads;
mod vault;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_screenshot;
#[cfg(test)]
mod tests_sync;
#[cfg(test)]
mod tests_vault;

use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};

use crate::syncer::Event;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            // The vault is loaded into memory before the window shows anything.
            // LEDGER_CONFIG_DIR moves the per-machine settings (vault location, screenshot hotkey) for testing.
            let config_dir = match std::env::var_os("LEDGER_CONFIG_DIR") {
                Some(dir) => dir.into(),
                None => app.path().app_config_dir()?,
            };
            let core = state::Core::open(config_dir.clone(), app.path().document_dir().ok())?;
            let sync = core.sync.clone();
            let core = Arc::new(Mutex::new(core));

            let handle = app.handle().clone();
            let syncer = syncer::spawn(
                core.clone(),
                Arc::new(move |event| {
                    let _ = match event {
                        Event::Status(status) => handle.emit("sync-status", status),
                        Event::VaultChanged => handle.emit("vault-changed", ()),
                    };
                }),
            );
            // If .env is filled in, this also starts with a sync.
            syncer.reconfigure(sync.as_ref());

            app.manage(state::App::new(core, syncer, screenshot::Screenshots::new(config_dir)));
            screenshot::register_saved(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::render_markdown,
            commands::search,
            commands::list_threads,
            commands::create_thread,
            commands::rename_thread,
            commands::get_thread,
            commands::delete_thread,
            commands::link_threads,
            commands::unlink_threads,
            commands::add_post,
            commands::update_post,
            commands::delete_post,
            commands::list_document_types,
            commands::create_document_type,
            commands::update_document_type,
            commands::delete_document_type,
            commands::list_documents,
            commands::get_document,
            commands::get_document_version,
            commands::create_document,
            commands::update_document,
            commands::publish_document,
            commands::start_drafting,
            commands::delete_document,
            commands::get_planboard,
            commands::create_user_story,
            commands::update_user_story,
            commands::delete_user_story,
            commands::link_story_thread,
            commands::unlink_story_thread,
            commands::add_column,
            commands::rename_column,
            commands::delete_column,
            commands::get_settings,
            commands::sync_status,
            commands::save_sync_settings,
            commands::disconnect_sync,
            commands::test_sync,
            commands::sync_now,
            commands::set_vault_path,
            commands::reload_vault,
            commands::check_vault_changes,
            commands::save_screenshot_settings,
            commands::take_screenshot,
            commands::pause_screenshot_hotkey,
            commands::screenshot_image,
            commands::screenshot_overlay_ready,
            commands::screenshot_finish,
            commands::screenshot_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
