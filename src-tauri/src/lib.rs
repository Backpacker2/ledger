mod board;
mod commands;
mod crypto;
mod db;
mod deliverables;
mod encrypted_remote;
mod error;
mod feedback;
mod frontmatter;
mod github;
mod ids;
mod keystore;
mod markdown;
mod models;
mod posts;
mod projects;
mod prompts;
mod screenshot;
mod search;
mod settings;
mod state;
mod stories;
mod sync;
mod syncer;
mod templates;
mod threads;
mod uploads;
mod vault;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_crypto;
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

            app.manage(state::App::new(core, syncer, screenshot::Screenshots::new()));
            screenshot::register_saved(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::render_markdown,
            commands::search,
            commands::list_projects,
            commands::create_project,
            commands::rename_project,
            commands::get_project,
            commands::save_readme,
            commands::delete_project,
            commands::create_thread,
            commands::rename_thread,
            commands::get_thread,
            commands::delete_thread,
            commands::link_threads,
            commands::unlink_threads,
            commands::add_post,
            commands::update_post,
            commands::delete_post,
            commands::create_prompt,
            commands::update_prompt,
            commands::delete_prompt,
            commands::link_prompt_deliverable,
            commands::unlink_prompt_deliverable,
            commands::create_prompt_deliverable,
            commands::add_feedback,
            commands::set_feedback_status,
            commands::update_feedback,
            commands::delete_feedback,
            commands::upload_files,
            commands::update_upload,
            commands::delete_upload,
            commands::read_project_file,
            commands::open_project_file,
            commands::list_deliverables,
            commands::get_deliverable,
            commands::create_deliverable,
            commands::save_deliverable,
            commands::publish_deliverable,
            commands::delete_deliverable,
            commands::list_templates,
            commands::project_templates,
            commands::effective_templates,
            commands::create_template,
            commands::update_template,
            commands::override_template,
            commands::delete_template,
            commands::list_stories,
            commands::create_story,
            commands::update_story,
            commands::delete_story,
            commands::link_story_thread,
            commands::unlink_story_thread,
            commands::get_board,
            commands::create_task,
            commands::update_task,
            commands::delete_task,
            commands::add_column,
            commands::rename_column,
            commands::delete_column,
            commands::get_settings,
            commands::sync_status,
            commands::save_sync_settings,
            commands::disconnect_sync,
            commands::test_sync,
            commands::sync_now,
            commands::encryption_status,
            commands::set_encryption_enabled,
            commands::generate_encryption_key,
            commands::import_encryption_key,
            commands::export_recovery_key,
            commands::set_vault_path,
            commands::reload_vault,
            commands::check_vault_changes,
            commands::save_screenshot_hotkey,
            commands::take_screenshot,
            commands::pause_screenshot_hotkey,
            commands::screenshot_image,
            commands::screenshot_overlay_ready,
            commands::screenshot_finish,
            commands::screenshot_cancel,
            commands::pending_screenshot_image,
            commands::attach_screenshot,
            commands::discard_screenshot,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
