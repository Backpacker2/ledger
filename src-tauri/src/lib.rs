mod activity;
mod applog;
mod board;
mod claude_code;
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
mod provenance;
mod screenshot;
mod search;
mod settings;
mod state;
mod stories;
mod sync;
mod syncer;
mod templates;
mod threads;
mod updates;
mod uploads;
mod vault;

// ---- Trello sync (feat/trello-sync) ----
mod trello;
mod trello_api;
mod trello_card;
mod trello_config;
mod trello_ledger;
mod trello_sync;
// ---- end Trello sync ----

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_applog;
#[cfg(test)]
mod tests_claude_code;
#[cfg(test)]
mod tests_trello;
#[cfg(test)]
mod tests_crypto;
#[cfg(test)]
mod tests_provenance;
#[cfg(test)]
mod tests_screenshot;
#[cfg(test)]
mod tests_sync;
#[cfg(test)]
mod tests_updates;
#[cfg(test)]
mod tests_vault;


use std::sync::{Arc, Mutex};

use tauri::{Emitter, Manager};

use crate::syncer::Event;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    // First, so a second launch hands its arguments over before anything else starts. That is how a picture
    // opened with Ledger (Flameshot's "Open with", a file manager) reaches the window that is already open.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
        screenshot::reveal_main(app);
        screenshot::open_files(app, args.get(1..).unwrap_or_default(), std::path::Path::new(&cwd));
    }));
    // Looking for and installing new releases (`updates.rs`): in release builds only, so `tauri dev`, debug builds
    // and sandboxes never look. `updates::BUILT_IN` says the same to the rest of the app.
    #[cfg(not(debug_assertions))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            // The vault is loaded into memory before the window shows anything.
            // LEDGER_CONFIG_DIR moves the per-machine settings (vault location, screenshot hotkey, update checks) for testing.
            let config_dir = match std::env::var_os("LEDGER_CONFIG_DIR") {
                Some(dir) => dir.into(),
                None => app.path().app_config_dir()?,
            };
            // The log first, so whatever goes wrong from here on is in it. LEDGER_CONFIG_DIR moves it along.
            let log_dir = match std::env::var_os("LEDGER_CONFIG_DIR") {
                Some(_) => config_dir.join("logs"),
                None => app.path().app_log_dir()?,
            };
            let log_handle = app.handle().clone();
            applog::init(
                &log_dir,
                settings::load_config(&config_dir).log_max_bytes.unwrap_or(applog::DEFAULT_MAX_BYTES),
                Box::new(move |entry| {
                    let _ = log_handle.emit("log-entry", entry);
                }),
            );
            applog::info("app", format!("Ledger {} started on {} {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH));
            let core = state::Core::open(config_dir.clone(), app.path().document_dir().ok())?;
            let sync = core.sync.clone();
            let core = Arc::new(Mutex::new(core));
            let trello_core = core.clone(); // for the Trello sync (feat/trello-sync), started below

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
            trello::start_app(app.handle(), trello_core); // feat/trello-sync: its own worker, needs the App state above
            // Looks for new releases now and then (release builds only); it asks the syncs above whether they are busy.
            updates::start(app.handle(), config_dir);
            screenshot::register_saved(app.handle());
            // Launched with a picture to attach: it waits until the window asks for it.
            let args: Vec<String> = std::env::args().skip(1).collect();
            screenshot::open_files(app.handle(), &args, &std::env::current_dir().unwrap_or_default());
            Ok(())
        })
        .on_window_event(claude_code::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::render_markdown,
            commands::search,
            commands::list_activity,
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
            commands::get_provenance,
            commands::update_version_record,
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
            applog::about_info,
            applog::log_write,
            applog::log_read,
            applog::log_clear,
            applog::log_set_max_bytes,
            applog::capture_status,
            applog::capture_start,
            applog::capture_debug,
            applog::capture_stop,
            applog::capture_save,
            // ---- Trello sync (feat/trello-sync) ----
            trello::trello_get,
            trello::trello_status,
            trello::trello_test,
            trello::trello_save_credentials,
            trello::trello_disconnect,
            trello::trello_boards,
            trello::trello_board_members,
            trello::trello_save_member,
            trello::trello_link_project,
            trello::trello_project_mapping,
            trello::trello_set_column_list,
            trello::trello_sync_now,
            trello::trello_relink_item,
            // ---- end Trello sync ----
            commands::set_vault_path,
            commands::reload_vault,
            commands::check_vault_changes,
            commands::save_screenshot_hotkey,
            commands::save_screenshot_tool,
            commands::pending_screenshot,
            commands::take_screenshot,
            commands::pause_screenshot_hotkey,
            commands::screenshot_image,
            commands::screenshot_overlay_ready,
            commands::screenshot_finish,
            commands::screenshot_cancel,
            commands::pending_screenshot_image,
            commands::attach_screenshot,
            commands::discard_screenshot,
            claude_code::claude_code_settings,
            claude_code::save_claude_code_settings,
            claude_code::check_claude_code,
            claude_code::claude_code_preview,
            claude_code::claude_code_start,
            claude_code::claude_code_cancel,
            claude_code::claude_code_runs,
            claude_code::claude_code_run,
            updates::update_status,
            updates::save_update_settings,
            updates::check_for_updates,
            updates::install_update,
            updates::simulate_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            // macOS doesn't pass opened files as arguments: they arrive here, at launch and while running.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &_event {
                let paths: Vec<String> = urls.iter().filter_map(|u| u.to_file_path().ok()).map(|p| p.display().to_string()).collect();
                screenshot::open_files(_app, &paths, std::path::Path::new("/"));
            }
        });
}
