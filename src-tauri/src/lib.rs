mod commands;
mod db;
mod document_types;
mod documents;
mod error;
mod ids;
mod markdown;
mod models;
mod planboard;
mod posts;
mod search;
mod stories;
mod threads;

#[cfg(test)]
mod tests;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // LEDGER_DATA_DIR moves the database (a synced folder, a throwaway dir for testing).
            let dir = match std::env::var_os("LEDGER_DATA_DIR") {
                Some(dir) => dir.into(),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&dir)?;
            let path = dir.join("ledger.db");
            let conn = db::open(&path)?;
            app.manage(db::Db::new(conn, path));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
