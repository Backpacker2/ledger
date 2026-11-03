//! The Tauri command surface: thin wrappers over the stores. Reads are answered from the in-memory database;
//! everything that changes something goes through `App::mutate`, which writes it through to the vault's files.
//! Argument names are snake_case here and camelCase from the frontend.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::github::GithubRemote;
use crate::keystore::{self, EncryptionStatus, KeyStore};
use crate::models::*;
use crate::posts::AddOptions;
use crate::screenshot::{self, Region, ScreenshotSettings};
use crate::settings::{self, SyncConfig, SyncEnv};
use crate::state::App;
use crate::syncer::SyncStatus;
use crate::{board, deliverables, feedback, markdown, posts, projects, prompts, search, stories, templates, threads, uploads};

#[tauri::command]
pub fn render_markdown(body: String) -> String {
    markdown::render(&body)
}

#[tauri::command]
pub fn search(app: State<App>, query: String) -> Result<Vec<SearchHit>> {
    app.read(|c, _| search::search(c, &query))
}

// ---- projects ---------------------------------------------------------------------------------------

/// Every project with its threads, for the sidebar.
#[tauri::command]
pub fn list_projects(app: State<App>) -> Result<Vec<ProjectSummary>> {
    app.read(|c, _| projects::list(c))
}

#[tauri::command]
pub fn create_project(app: State<App>, name: String) -> Result<ProjectSummary> {
    app.mutate(|c, _| projects::create(c, &name))
}

#[tauri::command]
pub fn rename_project(app: State<App>, slug: String, name: String) -> Result<ProjectSummary> {
    app.mutate(|c, _| projects::rename(c, &slug, &name))
}

#[tauri::command]
pub fn get_project(app: State<App>, slug: String) -> Result<ProjectView> {
    app.read(|c, _| projects::get(c, &slug))
}

#[tauri::command]
pub fn save_readme(app: State<App>, slug: String, text: String) -> Result<ProjectView> {
    app.mutate(|c, _| projects::save_readme(c, &slug, &text))
}

/// Deletes the project and its whole folder, attachments and non-markdown deliverables included.
#[tauri::command]
pub fn delete_project(app: State<App>, slug: String) -> Result<()> {
    {
        let mut guard = app.lock();
        let core = &mut *guard;
        safe_segment(&slug)?;
        projects::delete(&core.conn, &slug)?;
        core.vault.persist(&core.conn)?;
        core.vault.remove_project_dir(&slug)?;
    }
    app.syncer.dirty();
    Ok(())
}

// ---- threads ----------------------------------------------------------------------------------------

#[tauri::command]
pub fn create_thread(app: State<App>, project: String, title: String) -> Result<ThreadSummary> {
    app.mutate(|c, _| threads::create(c, &project, &title))
}

#[tauri::command]
pub fn rename_thread(app: State<App>, project: String, slug: String, title: String) -> Result<ThreadSummary> {
    app.mutate(|c, _| threads::rename(c, &project, &slug, &title))
}

#[tauri::command]
pub fn get_thread(app: State<App>, project: String, slug: String) -> Result<ThreadView> {
    app.read(|c, root| threads::get(c, root, &project, &slug))
}

#[tauri::command]
pub fn delete_thread(app: State<App>, project: String, slug: String) -> Result<()> {
    app.mutate(|c, root| threads::delete(c, root, &project, &slug))
}

#[tauri::command]
pub fn link_threads(app: State<App>, project: String, slug: String, other_slug: String) -> Result<()> {
    app.mutate(|c, _| threads::link(c, &project, &slug, &other_slug))
}

#[tauri::command]
pub fn unlink_threads(app: State<App>, project: String, slug: String, other_slug: String) -> Result<()> {
    app.mutate(|c, _| threads::unlink(c, &project, &slug, &other_slug))
}

// ---- posts ------------------------------------------------------------------------------------------

#[tauri::command]
pub fn add_post(app: State<App>, project: String, thread: String, body: String, reply_to_id: Option<String>) -> Result<PostView> {
    app.mutate(|c, _| posts::add(c, &project, &thread, &body, AddOptions { reply_to_id, ..Default::default() }))
}

#[tauri::command]
pub fn update_post(app: State<App>, project: String, thread: String, id: String, body: String) -> Result<PostView> {
    app.mutate(|c, _| posts::update(c, &project, &thread, &id, &body))
}

#[tauri::command]
pub fn delete_post(app: State<App>, project: String, thread: String, id: String) -> Result<()> {
    app.mutate(|c, _| posts::delete(c, &project, &thread, &id))
}

// ---- prompts ----------------------------------------------------------------------------------------

#[tauri::command]
pub fn create_prompt(
    app: State<App>,
    project: String,
    thread: String,
    title: String,
    body: String,
    template_id: Option<String>,
    deliverable: Option<String>,
) -> Result<PromptView> {
    app.mutate(|c, _| prompts::create_linked(c, &project, &thread, &title, &body, template_id.as_deref(), deliverable.as_deref()))
}

#[tauri::command]
pub fn update_prompt(app: State<App>, project: String, thread: String, id: String, title: Option<String>, body: Option<String>) -> Result<PromptView> {
    app.mutate(|c, _| prompts::update(c, &project, &thread, &id, title.as_deref(), body.as_deref()))
}

#[tauri::command]
pub fn delete_prompt(app: State<App>, project: String, thread: String, id: String) -> Result<()> {
    app.mutate(|c, _| prompts::delete(c, &project, &thread, &id))
}

#[tauri::command]
pub fn link_prompt_deliverable(app: State<App>, project: String, thread: String, id: String, path: String) -> Result<PromptView> {
    app.mutate(|c, _| prompts::link_deliverable(c, &project, &thread, &id, &path))
}

#[tauri::command]
pub fn unlink_prompt_deliverable(app: State<App>, project: String, thread: String, id: String, path: String) -> Result<PromptView> {
    app.mutate(|c, _| prompts::unlink_deliverable(c, &project, &thread, &id, &path))
}

/// Starts a deliverable for the prompt: a new markdown file (from the prompt's template, if it has one) that the
/// prompt is linked to.
#[tauri::command]
pub fn create_prompt_deliverable(app: State<App>, project: String, thread: String, id: String, name: String) -> Result<PromptView> {
    app.mutate(|c, _| {
        let prompt = prompts::get(c, &project, &thread, &id)?;
        let skeleton = prompt.template_id.as_deref().and_then(|t| templates::find(c, &project, t).ok()).map(|t| t.body).unwrap_or_default();
        let created = deliverables::create(c, &project, &name, &skeleton)?;
        prompts::link_deliverable(c, &project, &thread, &id, &created.path)
    })
}

// ---- feedback ---------------------------------------------------------------------------------------

#[tauri::command]
pub fn add_feedback(app: State<App>, project: String, thread: String, deliverable_path: String, body: String) -> Result<FeedbackView> {
    app.mutate(|c, _| feedback::add(c, &project, &thread, &deliverable_path, &body))
}

#[tauri::command]
pub fn set_feedback_status(app: State<App>, project: String, thread: String, id: String, status: String) -> Result<FeedbackView> {
    app.mutate(|c, _| feedback::set_status(c, &project, &thread, &id, &status))
}

#[tauri::command]
pub fn update_feedback(app: State<App>, project: String, thread: String, id: String, body: String) -> Result<FeedbackView> {
    app.mutate(|c, _| feedback::update(c, &project, &thread, &id, &body))
}

#[tauri::command]
pub fn delete_feedback(app: State<App>, project: String, thread: String, id: String) -> Result<()> {
    app.mutate(|c, _| feedback::delete(c, &project, &thread, &id))
}

// ---- uploads and project files ----------------------------------------------------------------------

/// Copies files from this machine into the project's attachments and records them in the thread.
#[tauri::command]
pub fn upload_files(app: State<App>, project: String, thread: String, paths: Vec<String>, description: String) -> Result<Vec<UploadView>> {
    let sources: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    app.mutate(|c, root| uploads::add_files(c, root, &project, &thread, &sources, &description))
}

#[tauri::command]
pub fn update_upload(app: State<App>, project: String, thread: String, id: String, description: String) -> Result<UploadView> {
    app.mutate(|c, root| uploads::set_description(c, root, &project, &thread, &id, &description))
}

#[tauri::command]
pub fn delete_upload(app: State<App>, project: String, thread: String, id: String) -> Result<()> {
    app.mutate(|c, root| uploads::delete(c, root, &project, &thread, &id))
}

/// A path inside a project's folder, from a path given relative to it; anything that could leave the folder is refused.
fn project_path(root: &Path, project: &str, rel: &str) -> Result<PathBuf> {
    safe_segment(project)?;
    let rel_path = Path::new(rel);
    if rel.is_empty() || rel_path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(Error::invalid("That path isn't inside the project"));
    }
    Ok(uploads::project_dir(root, project).join(rel_path))
}

fn safe_segment(segment: &str) -> Result<()> {
    if segment.is_empty() || segment == "." || segment == ".." || segment.contains(['/', '\\']) {
        return Err(Error::invalid("That isn't a project"));
    }
    Ok(())
}

/// The bytes of a file in a project's folder (an attachment, a deliverable), for the screen to show.
#[tauri::command]
pub fn read_project_file(app: State<App>, project: String, path: String) -> Result<tauri::ipc::Response> {
    let file = app.read(|_, root| project_path(root, &project, &path))?;
    let bytes = std::fs::read(&file).map_err(|e| Error::io(&file, e))?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Opens a file in a project's folder with the system's default program, or shows it in the file manager.
#[tauri::command]
pub fn open_project_file(app: AppHandle, project: String, path: String, reveal: bool) -> Result<()> {
    let state = app.state::<App>();
    let file = state.read(|_, root| project_path(root, &project, &path))?;
    if !file.exists() {
        return Err(Error::not_found("That file isn't there any more"));
    }
    let opener = app.opener();
    if reveal {
        opener.reveal_item_in_dir(&file).map_err(Error::screenshot)
    } else {
        opener.open_path(file.display().to_string(), None::<&str>).map_err(Error::screenshot)
    }
}

// ---- deliverables -----------------------------------------------------------------------------------

#[tauri::command]
pub fn list_deliverables(app: State<App>, project: String) -> Result<Vec<DeliverableSummary>> {
    app.read(|c, _| deliverables::list(c, &project))
}

#[tauri::command]
pub fn get_deliverable(app: State<App>, project: String, path: String) -> Result<DeliverableView> {
    app.read(|c, _| deliverables::get(c, &project, &path))
}

#[tauri::command]
pub fn create_deliverable(app: State<App>, project: String, name: String, body: String) -> Result<DeliverableView> {
    app.mutate(|c, _| deliverables::create(c, &project, &name, &body))
}

#[tauri::command]
pub fn save_deliverable(app: State<App>, project: String, path: String, body: String) -> Result<DeliverableView> {
    app.mutate(|c, _| deliverables::save(c, &project, &path, &body))
}

#[tauri::command]
pub fn publish_deliverable(app: State<App>, project: String, path: String, bump: Option<Bump>, version: Option<String>) -> Result<DeliverableView> {
    app.mutate(|c, root| deliverables::publish(c, root, &project, &path, bump.unwrap_or_default(), version.as_deref()))
}

#[tauri::command]
pub fn delete_deliverable(app: State<App>, project: String, path: String) -> Result<()> {
    app.mutate(|c, root| deliverables::delete(c, root, &project, &path))
}

// ---- templates --------------------------------------------------------------------------------------

/// The templates of one scope: "" for the vault-wide ones, or a project's slug.
#[tauri::command]
pub fn list_templates(app: State<App>, scope: String) -> Result<Vec<TemplateView>> {
    app.read(|c, _| templates::list(c, &scope))
}

/// A project's templates followed by the vault-wide ones (marked if the project replaced them).
#[tauri::command]
pub fn project_templates(app: State<App>, project: String) -> Result<Vec<TemplateView>> {
    app.read(|c, _| templates::for_project(c, &project))
}

/// What a prompt in the project can start from.
#[tauri::command]
pub fn effective_templates(app: State<App>, project: String) -> Result<Vec<TemplateView>> {
    app.read(|c, _| templates::effective(c, &project))
}

#[tauri::command]
pub fn create_template(app: State<App>, scope: String, name: String, trigger: String, questions: Vec<String>, body: String) -> Result<TemplateView> {
    app.mutate(|c, _| templates::create(c, &scope, &name, &trigger, &questions, &body))
}

/// Gives the project its own copy of a vault-wide template, which then replaces the shared one there.
#[tauri::command]
pub fn override_template(app: State<App>, project: String, id: String) -> Result<TemplateView> {
    app.mutate(|c, _| templates::override_global(c, &project, &id))
}

#[tauri::command]
pub fn update_template(
    app: State<App>,
    scope: String,
    id: String,
    name: Option<String>,
    trigger: Option<String>,
    questions: Option<Vec<String>>,
    body: Option<String>,
) -> Result<TemplateView> {
    app.mutate(|c, _| templates::update(c, &scope, &id, name.as_deref(), trigger.as_deref(), questions.as_deref(), body.as_deref()))
}

#[tauri::command]
pub fn delete_template(app: State<App>, scope: String, id: String) -> Result<()> {
    app.mutate(|c, _| templates::delete(c, &scope, &id))
}

// ---- user stories and the board ---------------------------------------------------------------------

#[tauri::command]
pub fn list_stories(app: State<App>, project: String) -> Result<Vec<StoryView>> {
    app.read(|c, _| stories::list(c, &project))
}

#[tauri::command]
pub fn create_story(app: State<App>, project: String, draft: StoryDraft) -> Result<StoryView> {
    app.mutate(|c, _| stories::create(c, &project, draft))
}

#[tauri::command]
pub fn update_story(app: State<App>, project: String, id: String, updates: StoryUpdate) -> Result<StoryView> {
    app.mutate(|c, _| stories::update(c, &project, &id, updates))
}

#[tauri::command]
pub fn delete_story(app: State<App>, project: String, id: String) -> Result<()> {
    app.mutate(|c, _| stories::delete(c, &project, &id))
}

#[tauri::command]
pub fn link_story_thread(app: State<App>, project: String, id: String, thread_slug: String) -> Result<StoryView> {
    app.mutate(|c, _| stories::add_thread(c, &project, &id, &thread_slug))
}

#[tauri::command]
pub fn unlink_story_thread(app: State<App>, project: String, id: String, thread_slug: String) -> Result<StoryView> {
    app.mutate(|c, _| stories::remove_thread(c, &project, &id, &thread_slug))
}

#[tauri::command]
pub fn get_board(app: State<App>, project: String) -> Result<Board> {
    app.read(|c, _| board::get(c, &project))
}

#[tauri::command]
pub fn create_task(app: State<App>, project: String, title: String, body: String, story_id: Option<String>, thread_slug: Option<String>) -> Result<TaskView> {
    app.mutate(|c, _| board::create_task(c, &project, &title, &body, story_id.as_deref(), thread_slug.as_deref()))
}

#[tauri::command]
pub fn update_task(app: State<App>, project: String, id: String, updates: TaskUpdate) -> Result<TaskView> {
    app.mutate(|c, _| board::update_task(c, &project, &id, updates))
}

#[tauri::command]
pub fn delete_task(app: State<App>, project: String, id: String) -> Result<()> {
    app.mutate(|c, _| board::delete_task(c, &project, &id))
}

#[tauri::command]
pub fn add_column(app: State<App>, project: String, name: String) -> Result<Vec<Column>> {
    app.mutate(|c, _| board::add_column(c, &project, &name))
}

#[tauri::command]
pub fn rename_column(app: State<App>, project: String, id: String, name: String) -> Result<Vec<Column>> {
    app.mutate(|c, _| board::rename_column(c, &project, &id, &name))
}

#[tauri::command]
pub fn delete_column(app: State<App>, project: String, id: String) -> Result<Vec<Column>> {
    app.mutate(|c, _| board::delete_column(c, &project, &id))
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
    /// Where the vault's settings and its secret are kept (both inside the vault, both gitignored).
    pub settings_file: String,
    pub secrets_file: String,
    pub version: String,
    pub sync: SyncSettings,
    pub status: SyncStatus,
    pub screenshot: ScreenshotSettings,
    /// Whether sync encrypts, and whether this machine has the key. Never the key.
    pub encryption: EncryptionStatus,
}

fn view(app: &App) -> SettingsView {
    let (root, locked, env, keys) = {
        let core = app.lock();
        let root = core.vault.root().to_path_buf();
        let env = settings::read_env(&root);
        (root, core.vault_from_env, env, core.keys.clone())
    };
    let token_hint = if env.token.chars().count() >= 8 {
        format!("…{}", env.token.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>())
    } else {
        String::new()
    };
    SettingsView {
        vault_path: root.display().to_string(),
        vault_locked: locked,
        settings_file: settings::settings_path(&root).display().to_string(),
        secrets_file: settings::secrets_path(&root).display().to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        sync: SyncSettings {
            configured: env.complete().is_some(),
            has_token: !env.token.is_empty(),
            token_hint,
            repo: env.repo.clone(),
            branch: env.branch.clone(),
        },
        status: app.syncer.status(),
        screenshot: app.screenshots.settings(&root),
        encryption: keystore::status(&root, &*keys),
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

/// Saves the repository and branch to the vault's settings and the token to its `.env`, and starts syncing. A
/// blank token keeps the one already saved.
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

/// Turns sync off. The repository and branch stay in the settings; the token is removed.
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

// ---- encrypting what goes to GitHub -----------------------------------------------------------------

/// Where the vault is and what holds its key, taken out of the core so the key store is asked without the vault
/// lock held.
fn key_context(app: &App) -> (PathBuf, Arc<dyn KeyStore>) {
    let core = app.lock();
    (core.vault.root().to_path_buf(), core.keys.clone())
}

#[tauri::command]
pub fn encryption_status(app: State<App>) -> EncryptionStatus {
    let (root, keys) = key_context(&app);
    keystore::status(&root, &*keys)
}

/// Turns encryption of what is synced on or off, and syncs so that it takes effect. It can't be turned on until
/// there is a key.
#[tauri::command]
pub fn set_encryption_enabled(app: State<App>, enabled: bool) -> Result<SettingsView> {
    let (root, keys) = key_context(&app);
    keystore::set_enabled(&root, &*keys, enabled)?;
    if app.lock().sync.is_some() {
        app.syncer.now();
    }
    Ok(view(&app))
}

/// Makes a new random key and keeps it in the vault's `.ledger/.env`. With a key already on this machine that
/// takes `replace`.
#[tauri::command]
pub fn generate_encryption_key(app: State<App>, replace: bool) -> Result<SettingsView> {
    let (root, keys) = key_context(&app);
    keystore::generate(&root, &*keys, replace)?;
    Ok(view(&app))
}

/// Keeps a key made on another machine: its recovery string, or the bare key as hex or base64.
#[tauri::command]
pub fn import_encryption_key(app: State<App>, key: String, replace: bool) -> Result<SettingsView> {
    let key = Zeroizing::new(key);
    let (root, keys) = key_context(&app);
    keystore::import(&root, &*keys, &key, replace)?;
    Ok(view(&app))
}

/// The recovery string for the key: the one command that hands the key out, and only when asked for.
#[tauri::command]
pub fn export_recovery_key(app: State<App>) -> Result<String> {
    let (root, keys) = key_context(&app);
    let recovery = keystore::export(&root, &*keys)?;
    Ok(String::clone(&recovery))
}

/// Moves the app to the vault in `path`. With `copy`, an empty destination first receives the current vault.
#[tauri::command]
pub fn set_vault_path(app: AppHandle, path: String, copy: bool) -> Result<SettingsView> {
    let state = app.state::<App>();
    let config = {
        let mut core = state.lock();
        core.set_vault(&PathBuf::from(path.trim()), copy)?;
        core.sync.clone()
    };
    state.syncer.reconfigure(config.as_ref());
    // The new vault has its own settings, the screenshot hotkey among them.
    screenshot::register_saved(&app);
    Ok(view(&state))
}

/// Reads the vault in again from its files.
#[tauri::command]
pub fn reload_vault(app: State<App>) -> Result<()> {
    app.lock().reload()?;
    app.syncer.dirty();
    Ok(())
}

/// Reloads if the files were changed behind the app's back (in Obsidian, say). Whether anything was reloaded.
#[tauri::command]
pub fn check_vault_changes(app: State<App>) -> Result<bool> {
    let mut core = app.lock();
    if !core.vault.changed_on_disk() {
        return Ok(false);
    }
    core.reload()?;
    // What was edited elsewhere is part of the vault now, and goes to GitHub like anything else.
    app.syncer.dirty();
    Ok(true)
}

// ---- screenshots ------------------------------------------------------------------------------------

/// Saves the screenshot hotkey in the vault's settings. It is registered first, replacing the old one live; if the
/// system refuses it nothing is saved and the old one keeps working.
#[tauri::command]
pub fn save_screenshot_hotkey(app: AppHandle, hotkey: String) -> Result<SettingsView> {
    screenshot::apply_hotkey(&app, &hotkey)?;
    let state = app.state::<App>();
    let root = state.lock().vault.root().to_path_buf();
    settings::update_settings(&root, |s| s.screenshot_hotkey = Some(hotkey.trim().to_string()))?;
    Ok(view(&state))
}

/// Chooses between the built-in capture and the user's own tool (and saves the tool's command either way).
#[tauri::command]
pub fn save_screenshot_tool(app: State<App>, use_command: bool, command: String) -> Result<SettingsView> {
    let root = app.lock().vault.root().to_path_buf();
    screenshot::save_tool(&root, use_command, &command)?;
    Ok(view(&app))
}

/// The screenshot waiting to be attached, if there is one (Ledger may have been launched with a picture).
#[tauri::command]
pub fn pending_screenshot(app: AppHandle) -> Option<screenshot::Taken> {
    screenshot::pending(&app)
}

/// Takes a screenshot as if the hotkey had been pressed.
#[tauri::command]
pub fn take_screenshot(app: AppHandle) {
    screenshot::trigger(&app);
}

/// Lets go of the hotkey while the settings screen records a new one (and takes it back afterwards).
#[tauri::command]
pub fn pause_screenshot_hotkey(app: AppHandle, paused: bool) {
    screenshot::pause_hotkey(&app, paused);
}

/// The overlay's picture of the screen, as raw PNG bytes.
#[tauri::command]
pub fn screenshot_image(app: AppHandle) -> Result<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(screenshot::overlay_image(&app)?))
}

#[tauri::command]
pub fn screenshot_overlay_ready(app: AppHandle) {
    screenshot::overlay_ready(&app);
}

#[tauri::command]
pub fn screenshot_finish(app: AppHandle, region: Region) -> Result<()> {
    screenshot::finish(&app, region)
}

#[tauri::command]
pub fn screenshot_cancel(app: AppHandle) {
    screenshot::cancel(&app);
}

/// The screenshot waiting to be attached, as PNG bytes.
#[tauri::command]
pub fn pending_screenshot_image(app: AppHandle) -> Result<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(screenshot::pending_image(&app)?))
}

/// Saves the waiting screenshot in the project's attachments and records it in the thread.
#[tauri::command]
pub fn attach_screenshot(app: AppHandle, project: String, thread: String, description: String) -> Result<UploadView> {
    let state = app.state::<App>();
    let png = screenshot::pending_image(&app)?;
    let upload = state.mutate(|c, root| uploads::add_bytes(c, root, &project, &thread, &screenshot::file_name(), &png, &description))?;
    screenshot::discard_pending(&app);
    Ok(upload)
}

#[tauri::command]
pub fn discard_screenshot(app: AppHandle) {
    screenshot::discard_pending(&app);
}
