//! The Tauri command surface: thin wrappers over the stores. Reads are answered from the in-memory database;
//! everything that changes something goes through `App::mutate`, which writes it through to the vault's files.
//! Argument names are snake_case here and camelCase from the frontend.

use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::error::{Error, Result};
use crate::github::GithubRemote;
use crate::models::*;
use crate::posts::AddOptions;
use crate::settings::{self, SyncConfig, SyncEnv};
use crate::state::App;
use crate::syncer::SyncStatus;
use crate::{document_types, documents, markdown, planboard, posts, search, stories, threads};

#[tauri::command]
pub fn render_markdown(body: String) -> String {
    markdown::render(&body)
}

#[tauri::command]
pub fn search(app: State<App>, query: String) -> Result<Vec<SearchHit>> {
    app.read(|c| search::search(c, &query))
}

// ---- threads ----------------------------------------------------------------------------------------

#[tauri::command]
pub fn list_threads(app: State<App>) -> Result<Vec<ThreadSummary>> {
    app.read(threads::list)
}

#[tauri::command]
pub fn create_thread(app: State<App>, title: String) -> Result<ThreadSummary> {
    app.mutate(|c| threads::create(c, &title))
}

#[tauri::command]
pub fn rename_thread(app: State<App>, slug: String, title: String) -> Result<ThreadSummary> {
    app.mutate(|c| threads::rename(c, &slug, &title))
}

#[tauri::command]
pub fn get_thread(app: State<App>, slug: String) -> Result<ThreadView> {
    app.read(|c| threads::get(c, &slug))
}

#[tauri::command]
pub fn delete_thread(app: State<App>, slug: String) -> Result<()> {
    app.mutate(|c| threads::delete(c, &slug))
}

#[tauri::command]
pub fn link_threads(app: State<App>, slug: String, other_slug: String) -> Result<()> {
    app.mutate(|c| threads::link(c, &slug, &other_slug))
}

#[tauri::command]
pub fn unlink_threads(app: State<App>, slug: String, other_slug: String) -> Result<()> {
    app.mutate(|c| threads::unlink(c, &slug, &other_slug))
}

// ---- posts ------------------------------------------------------------------------------------------

#[tauri::command]
pub fn add_post(app: State<App>, slug: String, body: String, reply_to_id: Option<String>) -> Result<PostView> {
    app.mutate(|c| posts::add(c, &slug, &body, AddOptions { reply_to_id, ..Default::default() }))
}

#[tauri::command]
pub fn update_post(app: State<App>, slug: String, id: String, body: String) -> Result<PostView> {
    app.mutate(|c| posts::update(c, &slug, &id, &body))
}

#[tauri::command]
pub fn delete_post(app: State<App>, slug: String, id: String) -> Result<()> {
    app.mutate(|c| posts::delete(c, &slug, &id))
}

// ---- document types ---------------------------------------------------------------------------------

#[tauri::command]
pub fn list_document_types(app: State<App>) -> Result<Vec<DocumentType>> {
    app.read(document_types::list)
}

#[tauri::command]
pub fn create_document_type(app: State<App>, name: String, trigger: Option<String>, questions: Option<Vec<String>>) -> Result<DocumentType> {
    app.mutate(|c| document_types::create(c, &name, trigger.as_deref(), questions.as_deref()))
}

#[tauri::command]
pub fn update_document_type(
    app: State<App>,
    slug: String,
    name: Option<String>,
    trigger: Option<String>,
    questions: Option<Vec<String>>,
) -> Result<DocumentType> {
    app.mutate(|c| document_types::update(c, &slug, name.as_deref(), trigger.as_deref(), questions.as_deref()))
}

#[tauri::command]
pub fn delete_document_type(app: State<App>, slug: String) -> Result<()> {
    app.mutate(|c| document_types::delete(c, &slug))
}

// ---- documents --------------------------------------------------------------------------------------

#[tauri::command]
pub fn list_documents(app: State<App>) -> Result<Vec<DocumentSummary>> {
    app.read(documents::list_all)
}

#[tauri::command]
pub fn get_document(app: State<App>, id: String) -> Result<DocumentView> {
    app.read(|c| documents::get(c, &id))
}

#[tauri::command]
pub fn get_document_version(app: State<App>, id: String, version: String) -> Result<DocumentView> {
    app.read(|c| documents::get_version(c, &id, &version))
}

#[tauri::command]
pub fn create_document(app: State<App>, document_type_slug: String, title: String, thread_slug: Option<String>) -> Result<DocumentView> {
    app.mutate(|c| documents::create(c, &document_type_slug, &title, thread_slug.as_deref()))
}

#[tauri::command]
pub fn update_document(app: State<App>, id: String, title: Option<String>, body: Option<String>) -> Result<DocumentView> {
    app.mutate(|c| documents::update(c, &id, title.as_deref(), body.as_deref()))
}

#[tauri::command]
pub fn publish_document(app: State<App>, id: String, bump: Option<Bump>, version: Option<String>) -> Result<DocumentView> {
    app.mutate(|c| documents::publish(c, &id, bump.unwrap_or_default(), version.as_deref()))
}

#[tauri::command]
pub fn start_drafting(app: State<App>, id: String, thread_slug: Option<String>, new_thread_title: Option<String>) -> Result<DocumentView> {
    app.mutate(|c| documents::start_drafting(c, &id, thread_slug.as_deref(), new_thread_title.as_deref()))
}

#[tauri::command]
pub fn delete_document(app: State<App>, id: String) -> Result<()> {
    app.mutate(|c| documents::delete(c, &id))
}

// ---- user stories and the planboard -----------------------------------------------------------------

#[tauri::command]
pub fn get_planboard(app: State<App>) -> Result<Planboard> {
    app.read(planboard::get)
}

#[tauri::command]
pub fn create_user_story(app: State<App>, draft: StoryDraft) -> Result<StoryView> {
    app.mutate(|c| stories::create(c, draft))
}

#[tauri::command]
pub fn update_user_story(app: State<App>, id: String, updates: StoryUpdate) -> Result<StoryView> {
    app.mutate(|c| stories::update(c, &id, updates))
}

#[tauri::command]
pub fn delete_user_story(app: State<App>, id: String) -> Result<()> {
    app.mutate(|c| stories::delete(c, &id))
}

#[tauri::command]
pub fn link_story_thread(app: State<App>, id: String, thread_slug: String) -> Result<StoryView> {
    app.mutate(|c| stories::add_thread(c, &id, &thread_slug))
}

#[tauri::command]
pub fn unlink_story_thread(app: State<App>, id: String, thread_slug: String) -> Result<StoryView> {
    app.mutate(|c| stories::remove_thread(c, &id, &thread_slug))
}

#[tauri::command]
pub fn add_column(app: State<App>, name: String) -> Result<Vec<Column>> {
    app.mutate(|c| planboard::add_column(c, &name))
}

#[tauri::command]
pub fn rename_column(app: State<App>, id: String, name: String) -> Result<Vec<Column>> {
    app.mutate(|c| planboard::rename_column(c, &id, &name))
}

#[tauri::command]
pub fn delete_column(app: State<App>, id: String) -> Result<Vec<Column>> {
    app.mutate(|c| planboard::delete_column(c, &id))
}

// ---- the vault and GitHub sync ----------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSettings {
    pub repo: String,
    pub branch: String,
    /// The token itself never leaves the backend; the screen only learns that there is one.
    pub has_token: bool,
    pub token_hint: String,
    pub configured: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub vault_path: String,
    /// The location is fixed by the `LEDGER_VAULT` environment variable.
    pub vault_locked: bool,
    pub env_file: String,
    pub version: String,
    pub sync: SyncSettings,
    pub status: SyncStatus,
}

fn view(app: &App) -> SettingsView {
    let (root, locked, env) = {
        let core = app.lock();
        let root = core.vault.root().to_path_buf();
        let env = settings::read_env(&root);
        (root, core.vault_from_env, env)
    };
    let token_hint = if env.token.chars().count() >= 8 {
        format!("…{}", env.token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>())
    } else {
        String::new()
    };
    SettingsView {
        vault_path: root.display().to_string(),
        vault_locked: locked,
        env_file: root.join(settings::ENV_FILE).display().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        sync: SyncSettings {
            configured: env.complete().is_some(),
            has_token: !env.token.is_empty(),
            token_hint,
            repo: env.repo,
            branch: env.branch,
        },
        status: app.syncer.status(),
    }
}

#[tauri::command]
pub fn get_settings(app: State<App>) -> SettingsView {
    view(&app)
}

#[tauri::command]
pub fn sync_status(app: State<App>) -> SyncStatus {
    app.syncer.status()
}

/// Saves the repository, branch and token to the vault's `.env` and starts syncing. A blank token keeps the
/// one already saved.
#[tauri::command]
pub fn save_sync_settings(app: State<App>, repo: String, branch: String, token: Option<String>) -> Result<SettingsView> {
    let repo = settings::normalize_repo(&repo)?;
    let branch = settings::normalize_branch(&branch)?;

    let config = {
        let mut core = app.lock();
        let root = core.vault.root().to_path_buf();
        let token = token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| settings::read_env(&root).token);
        if token.is_empty() {
            return Err(Error::invalid("Enter an access token"));
        }
        settings::write_env(&root, &SyncEnv { repo, branch, token })?;
        core.sync = settings::read_env(&root).complete();
        core.sync.clone()
    };
    app.syncer.reconfigure(config.as_ref());
    Ok(view(&app))
}

/// Turns sync off. The repository and branch stay in `.env`; the token is removed.
#[tauri::command]
pub fn disconnect_sync(app: State<App>) -> Result<SettingsView> {
    {
        let mut core = app.lock();
        let root = core.vault.root().to_path_buf();
        let env = settings::read_env(&root);
        settings::write_env(&root, &SyncEnv { token: String::new(), ..env })?;
        core.sync = None;
    }
    app.syncer.reconfigure(None);
    Ok(view(&app))
}

/// Tries the connection with the values on screen (a blank token means the saved one) without saving anything.
#[tauri::command]
pub async fn test_sync(app: State<'_, App>, repo: String, branch: String, token: Option<String>) -> Result<String> {
    let repo = settings::normalize_repo(&repo)?;
    let branch = settings::normalize_branch(&branch)?;
    let token = {
        let core = app.lock();
        token
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| settings::read_env(core.vault.root()).token)
    };
    if token.is_empty() {
        return Err(Error::invalid("Enter an access token"));
    }

    let config = SyncConfig { repo: repo.clone(), branch: branch.clone(), token };
    let check = tauri::async_runtime::spawn_blocking(move || GithubRemote::new(&config)?.check())
        .await
        .map_err(|e| Error::sync(format!("The connection test stopped unexpectedly: {e}")))??;
    if !check.private {
        return Err(Error::sync(format!("{repo} is a public repository. Ledger only syncs to private repositories.")));
    }
    if !check.can_push {
        return Err(Error::sync("The token can read this repository but not write to it. It needs write access to its contents."));
    }
    Ok(if check.branch_exists {
        format!("Connected to {repo}. The branch {branch} exists.")
    } else {
        format!("Connected to {repo}. The branch {branch} doesn't exist yet and will be created on the first sync (the default branch is {}).", check.default_branch)
    })
}

#[tauri::command]
pub fn sync_now(app: State<App>) -> Result<()> {
    if app.lock().sync.is_none() {
        return Err(Error::invalid("GitHub sync isn't set up yet"));
    }
    app.syncer.now();
    Ok(())
}

/// Moves the app to the vault in `path`. With `copy`, an empty destination first receives the current vault.
#[tauri::command]
pub fn set_vault_path(app: State<App>, path: String, copy: bool) -> Result<SettingsView> {
    let config = {
        let mut core = app.lock();
        core.set_vault(&PathBuf::from(path.trim()), copy)?;
        core.sync.clone()
    };
    app.syncer.reconfigure(config.as_ref());
    Ok(view(&app))
}

/// Reads the vault in again from its files.
#[tauri::command]
pub fn reload_vault(app: State<App>) -> Result<()> {
    app.lock().reload()
}

/// Reloads if the files were changed behind the app's back (in Obsidian, say). Whether anything was reloaded.
#[tauri::command]
pub fn check_vault_changes(app: State<App>) -> Result<bool> {
    let mut core = app.lock();
    if !core.vault.changed_on_disk() {
        return Ok(false);
    }
    core.reload()?;
    Ok(true)
}
