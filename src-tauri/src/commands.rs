//! The Tauri command surface: thin wrappers that lock the connection and call into the stores. Argument names
//! are snake_case here and camelCase from the frontend.

use tauri::State;

use crate::db::Db;
use crate::error::Result;
use crate::models::*;
use crate::posts::AddOptions;
use crate::{document_types, documents, markdown, planboard, posts, search, stories, threads};

#[tauri::command]
pub fn app_info(db: State<Db>) -> AppInfo {
    AppInfo { version: env!("CARGO_PKG_VERSION").to_string(), db_path: db.path().display().to_string() }
}

#[tauri::command]
pub fn render_markdown(body: String) -> String {
    markdown::render(&body)
}

#[tauri::command]
pub fn search(db: State<Db>, query: String) -> Result<Vec<SearchHit>> {
    search::search(&db.conn(), &query)
}

// ---- threads ----------------------------------------------------------------------------------------

#[tauri::command]
pub fn list_threads(db: State<Db>) -> Result<Vec<ThreadSummary>> {
    threads::list(&db.conn())
}

#[tauri::command]
pub fn create_thread(db: State<Db>, title: String) -> Result<ThreadSummary> {
    threads::create(&db.conn(), &title)
}

#[tauri::command]
pub fn rename_thread(db: State<Db>, slug: String, title: String) -> Result<ThreadSummary> {
    threads::rename(&db.conn(), &slug, &title)
}

#[tauri::command]
pub fn get_thread(db: State<Db>, slug: String) -> Result<ThreadView> {
    threads::get(&db.conn(), &slug)
}

#[tauri::command]
pub fn delete_thread(db: State<Db>, slug: String) -> Result<()> {
    threads::delete(&db.conn(), &slug)
}

#[tauri::command]
pub fn link_threads(db: State<Db>, slug: String, other_slug: String) -> Result<()> {
    threads::link(&db.conn(), &slug, &other_slug)
}

#[tauri::command]
pub fn unlink_threads(db: State<Db>, slug: String, other_slug: String) -> Result<()> {
    threads::unlink(&db.conn(), &slug, &other_slug)
}

// ---- posts ------------------------------------------------------------------------------------------

#[tauri::command]
pub fn add_post(db: State<Db>, slug: String, body: String, reply_to_id: Option<String>) -> Result<PostView> {
    posts::add(&db.conn(), &slug, &body, AddOptions { reply_to_id, ..Default::default() })
}

#[tauri::command]
pub fn update_post(db: State<Db>, slug: String, id: String, body: String) -> Result<PostView> {
    posts::update(&db.conn(), &slug, &id, &body)
}

#[tauri::command]
pub fn delete_post(db: State<Db>, slug: String, id: String) -> Result<()> {
    posts::delete(&db.conn(), &slug, &id)
}

// ---- document types ---------------------------------------------------------------------------------

#[tauri::command]
pub fn list_document_types(db: State<Db>) -> Result<Vec<DocumentType>> {
    document_types::list(&db.conn())
}

#[tauri::command]
pub fn create_document_type(db: State<Db>, name: String, trigger: Option<String>, questions: Option<Vec<String>>) -> Result<DocumentType> {
    document_types::create(&db.conn(), &name, trigger.as_deref(), questions.as_deref())
}

#[tauri::command]
pub fn update_document_type(
    db: State<Db>,
    slug: String,
    name: Option<String>,
    trigger: Option<String>,
    questions: Option<Vec<String>>,
) -> Result<DocumentType> {
    document_types::update(&db.conn(), &slug, name.as_deref(), trigger.as_deref(), questions.as_deref())
}

#[tauri::command]
pub fn delete_document_type(db: State<Db>, slug: String) -> Result<()> {
    document_types::delete(&db.conn(), &slug)
}

// ---- documents --------------------------------------------------------------------------------------

#[tauri::command]
pub fn list_documents(db: State<Db>) -> Result<Vec<DocumentSummary>> {
    documents::list_all(&db.conn())
}

#[tauri::command]
pub fn get_document(db: State<Db>, id: String) -> Result<DocumentView> {
    documents::get(&db.conn(), &id)
}

#[tauri::command]
pub fn get_document_version(db: State<Db>, id: String, version: String) -> Result<DocumentView> {
    documents::get_version(&db.conn(), &id, &version)
}

#[tauri::command]
pub fn create_document(db: State<Db>, document_type_slug: String, title: String, thread_slug: Option<String>) -> Result<DocumentView> {
    documents::create(&db.conn(), &document_type_slug, &title, thread_slug.as_deref())
}

#[tauri::command]
pub fn update_document(db: State<Db>, id: String, title: Option<String>, body: Option<String>) -> Result<DocumentView> {
    documents::update(&db.conn(), &id, title.as_deref(), body.as_deref())
}

#[tauri::command]
pub fn publish_document(db: State<Db>, id: String, bump: Option<Bump>, version: Option<String>) -> Result<DocumentView> {
    documents::publish(&db.conn(), &id, bump.unwrap_or_default(), version.as_deref())
}

#[tauri::command]
pub fn start_drafting(db: State<Db>, id: String, thread_slug: Option<String>, new_thread_title: Option<String>) -> Result<DocumentView> {
    documents::start_drafting(&db.conn(), &id, thread_slug.as_deref(), new_thread_title.as_deref())
}

#[tauri::command]
pub fn delete_document(db: State<Db>, id: String) -> Result<()> {
    documents::delete(&db.conn(), &id)
}

// ---- user stories and the planboard -----------------------------------------------------------------

#[tauri::command]
pub fn get_planboard(db: State<Db>) -> Result<Planboard> {
    planboard::get(&db.conn())
}

#[tauri::command]
pub fn create_user_story(db: State<Db>, draft: StoryDraft) -> Result<StoryView> {
    stories::create(&db.conn(), draft)
}

#[tauri::command]
pub fn update_user_story(db: State<Db>, id: String, updates: StoryUpdate) -> Result<StoryView> {
    stories::update(&db.conn(), &id, updates)
}

#[tauri::command]
pub fn delete_user_story(db: State<Db>, id: String) -> Result<()> {
    stories::delete(&db.conn(), &id)
}

#[tauri::command]
pub fn link_story_thread(db: State<Db>, id: String, thread_slug: String) -> Result<StoryView> {
    stories::add_thread(&db.conn(), &id, &thread_slug)
}

#[tauri::command]
pub fn unlink_story_thread(db: State<Db>, id: String, thread_slug: String) -> Result<StoryView> {
    stories::remove_thread(&db.conn(), &id, &thread_slug)
}

#[tauri::command]
pub fn add_column(db: State<Db>, name: String) -> Result<Vec<Column>> {
    planboard::add_column(&db.conn(), &name)
}

#[tauri::command]
pub fn rename_column(db: State<Db>, id: String, name: String) -> Result<Vec<Column>> {
    planboard::rename_column(&db.conn(), &id, &name)
}

#[tauri::command]
pub fn delete_column(db: State<Db>, id: String) -> Result<Vec<Column>> {
    planboard::delete_column(&db.conn(), &id)
}
