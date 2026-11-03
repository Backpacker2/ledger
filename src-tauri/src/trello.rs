//! Syncing with Trello: the settings screen's commands and the background worker.
//!
//! The worker works like the GitHub one (`syncer.rs`): shortly after the vault changes (once it has been quiet
//! for a few seconds), every few minutes to notice what changed on Trello, and when asked; it reports its
//! progress through an event, and does nothing at all until the sync is set up. It never holds the vault lock while
//! it talks to Trello (see `trello_ledger.rs`).
//!
//! Setting up takes four things, in this order: the API key and token, the person whose cards are synced (picked
//! from a board's members), a Trello board for each project that should sync, and optionally which list a column
//! belongs to. A project with no board isn't synced. The key and token are kept in the vault's `.ledger/.env`; the
//! rest in `.ledger/trello.json` (see `trello_config.rs`). None of it is ever uploaded, and the screen is only
//! ever told whether a key and token exist, plus the last four characters of each.
//!
//! The two syncs can't echo each other. A Trello round writes into the vault, which the GitHub sync then pushes;
//! that is the end of it, because what a round writes is a function of the card (an imported item's id, its
//! timestamps and its text all come from the card), and a round that finds both sides agreeing writes nothing. A
//! pull from GitHub is noticed by the next round, which finds the cards already in step and does nothing.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{Error, Result};
use crate::models::Column;
use crate::state::{App, Core};
use crate::trello_api::{Board, HttpTrello, List, Member, ReqwestWire, TResult, Trello, TrelloError, valid_credential};
use crate::trello_card::Kind;
use crate::trello_config::{self as config, Credentials, ProjectLink};
use crate::trello_ledger::{self, CoreLedger, Ledger};
use crate::trello_sync::{self, Mapping, Report};
use crate::{ids, projects};

// ---- status -------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrelloStatus {
    /// There are credentials, a person and at least one linked project.
    pub configured: bool,
    /// "disabled", "idle", "syncing" or "error".
    pub state: String,
    /// Changes made here that haven't been looked at yet.
    pub pending: bool,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub summary: Option<String>,
    /// What the last round did, with the things it wants a person to know.
    pub report: Option<Report>,
}

pub enum TrelloEvent {
    Status(TrelloStatus),
    /// A round changed the vault: screens should refresh.
    VaultChanged,
}

pub type Emit = Arc<dyn Fn(TrelloEvent) + Send + Sync>;

/// How the worker is paced. Tests use short ones.
#[derive(Clone, Copy)]
pub struct Timing {
    /// How long the vault must stay quiet before a round, and the longest a steady stream of edits can postpone it.
    pub debounce: Duration,
    pub max_debounce: Duration,
    /// How often to look for changes made on Trello, and how soon to try again after a failure.
    pub poll: Duration,
    pub retry: Duration,
    /// The shortest wait after Trello said to slow down.
    pub rate_limited: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            debounce: Duration::from_secs(5),
            max_debounce: Duration::from_secs(60),
            poll: Duration::from_secs(180),
            retry: Duration::from_secs(60),
            rate_limited: Duration::from_secs(120),
        }
    }
}

/// Makes the client for a round from the saved credentials. Tests supply a fake one.
pub type Connect = Arc<dyn Fn(&Credentials) -> TResult<Box<dyn Trello>> + Send + Sync>;

fn connect_to_trello() -> Connect {
    Arc::new(|credentials| Ok(Box::new(HttpTrello::new(&credentials.api_key, &credentials.token)?)))
}

// ---- the worker ---------------------------------------------------------------------------------------

enum Msg {
    Dirty,
    Now,
}

pub struct TrelloSyncer {
    tx: Mutex<Sender<Msg>>,
    status: Arc<Mutex<TrelloStatus>>,
    emit: Emit,
}

/// How the rest of the app tells the worker the vault changed, without having to hold on to it.
static NOTIFY: OnceLock<Mutex<Sender<Msg>>> = OnceLock::new();

/// Something in the vault changed (called after every change made through the app). Does nothing when the Trello
/// sync isn't running.
pub fn notify_vault_changed() {
    if let Some(tx) = NOTIFY.get() {
        let _ = tx.lock().unwrap_or_else(PoisonError::into_inner).send(Msg::Dirty);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn update(status: &Mutex<TrelloStatus>, emit: &Emit, change: impl FnOnce(&mut TrelloStatus)) {
    let snapshot = {
        let mut guard = lock(status);
        change(&mut guard);
        guard.clone()
    };
    emit(TrelloEvent::Status(snapshot));
}

impl TrelloSyncer {
    /// Something in the vault changed (the app reaches the worker through [`notify_vault_changed`]).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn dirty(&self) {
        let _ = lock(&self.tx).send(Msg::Dirty);
    }

    /// Sync right away.
    pub fn now(&self) {
        let _ = lock(&self.tx).send(Msg::Now);
    }

    pub fn status(&self) -> TrelloStatus {
        lock(&self.status).clone()
    }

    /// The settings changed: records whether there is something to sync and, if there is, starts with a round.
    pub fn reconfigure(&self, root: &Path) {
        let configured = config::is_configured(root);
        update(&self.status, &self.emit, |s| {
            s.configured = configured;
            s.pending = false;
            s.last_error = None;
            s.state = if configured { "idle" } else { "disabled" }.to_string();
            if !configured {
                s.summary = None;
                s.last_sync_at = None;
                s.report = None;
            }
        });
        if configured {
            self.now();
        }
    }
}

/// Starts the worker for the running app and makes it the one [`notify_vault_changed`] reaches.
pub fn start(core: Arc<Mutex<Core>>, emit: Emit, changed: Arc<dyn Fn() + Send + Sync>) -> TrelloSyncer {
    let syncer = spawn(core.clone(), emit, changed, connect_to_trello(), Timing::default());
    let _ = NOTIFY.set(Mutex::new(lock(&syncer.tx).clone()));
    let root = lock(&core).vault.root().to_path_buf();
    syncer.reconfigure(&root);
    syncer
}

/// What `lib.rs` calls: starts the worker, reports to the window, and keeps the handle as app state.
pub fn start_app(handle: &AppHandle, core: Arc<Mutex<Core>>) {
    let emitter = handle.clone();
    let notifier = handle.clone();
    let syncer = start(
        core,
        Arc::new(move |event| {
            let _ = match event {
                TrelloEvent::Status(status) => emitter.emit("trello-status", status),
                TrelloEvent::VaultChanged => emitter.emit("vault-changed", ()),
            };
        }),
        // What a round wrote into the vault goes to GitHub like any other change.
        Arc::new(move || notifier.state::<App>().syncer.dirty()),
    );
    handle.manage(syncer);
}

pub fn spawn(core: Arc<Mutex<Core>>, emit: Emit, changed: Arc<dyn Fn() + Send + Sync>, connect: Connect, timing: Timing) -> TrelloSyncer {
    let (tx, rx) = channel();
    let status = Arc::new(Mutex::new(TrelloStatus { state: "disabled".into(), ..TrelloStatus::default() }));
    {
        let (status, emit) = (status.clone(), emit.clone());
        std::thread::spawn(move || worker(rx, core, status, emit, changed, connect, timing));
    }
    TrelloSyncer { tx: Mutex::new(tx), status, emit }
}

fn worker(rx: Receiver<Msg>, core: Arc<Mutex<Core>>, status: Arc<Mutex<TrelloStatus>>, emit: Emit, changed: Arc<dyn Fn() + Send + Sync>, connect: Connect, timing: Timing) {
    let mut pending_since: Option<Instant> = None;
    // How long to wait before trying again after a failure.
    let mut retry_in: Option<Duration> = None;
    // What the vault looked like (as far as Trello is concerned) at the last round that worked.
    let mut last_digest: Option<u64> = None;

    loop {
        let wait = if pending_since.is_some() { timing.debounce } else { retry_in.unwrap_or(timing.poll) };

        // `forced`: a round that must look at Trello even if the vault looks the same as last time.
        let (run, forced) = match rx.recv_timeout(wait) {
            Ok(Msg::Dirty) => {
                let root = lock(&core).vault.root().to_path_buf();
                if config::is_configured(&root) {
                    let since = *pending_since.get_or_insert_with(Instant::now);
                    update(&status, &emit, |s| s.pending = true);
                    (since.elapsed() >= timing.max_debounce, false)
                } else {
                    (false, false)
                }
            }
            Ok(Msg::Now) => (true, true),
            Err(RecvTimeoutError::Timeout) => (true, pending_since.is_none()),
            Err(RecvTimeoutError::Disconnected) => return,
        };

        if run {
            pending_since = None;
            retry_in = match run_once(&core, &status, &emit, &changed, &connect, forced, &mut last_digest) {
                Ok(()) => None,
                Err(TrelloError::RateLimited { retry_after }) => Some(retry_after.max(timing.rate_limited)),
                Err(_) => Some(timing.retry),
            };
        }
    }
}

/// One round. An error has already been put in the status.
fn run_once(
    core: &Arc<Mutex<Core>>,
    status: &Mutex<TrelloStatus>,
    emit: &Emit,
    changed: &Arc<dyn Fn() + Send + Sync>,
    connect: &Connect,
    forced: bool,
    last_digest: &mut Option<u64>,
) -> TResult<()> {
    let root = lock(core).vault.root().to_path_buf();
    let settings = config::read_settings(&root);
    let credentials = config::read_credentials(&root);
    if !config::is_configured(&root) {
        update(status, emit, |s| {
            s.configured = false;
            s.state = "disabled".into();
            s.pending = false;
        });
        return Ok(());
    }

    let ledger = CoreLedger { core: core.clone(), changed: changed.clone() };
    let result: TResult<Option<Report>> = (|| {
        let snapshot = ledger.snapshot()?;
        let digest = trello_sync::digest(&snapshot, &settings);
        if !forced && *last_digest == Some(digest) {
            // Nothing a round looks at has changed here since the last one: no need to bother Trello.
            update(status, emit, |s| s.pending = false);
            return Ok(None);
        }
        update(status, emit, |s| {
            s.configured = true;
            s.state = "syncing".into();
        });
        let client = connect(&credentials)?;
        let report = trello_sync::run(&root, &*client, &ledger, &settings, &snapshot)?;
        *last_digest = Some(digest);
        Ok(Some(report))
    })();

    match result {
        Ok(None) => Ok(()),
        Ok(Some(report)) => {
            let vault_changed = report.vault_changed();
            update(status, emit, |s| {
                s.configured = true;
                s.state = "idle".into();
                s.pending = false;
                if s.last_error.take().is_some() {
                    crate::applog::info("trello", "Syncing works again");
                }
                s.last_sync_at = Some(ids::now());
                s.summary = Some(report.summary());
                crate::applog::debug("trello", format!("Round done: {}", s.summary.as_deref().unwrap_or("no summary")));
                s.report = Some(report);
            });
            if vault_changed {
                emit(TrelloEvent::VaultChanged);
            }
            Ok(())
        }
        Err(e) => {
            update(status, emit, |s| {
                s.state = "error".into();
                let message = e.to_string();
                // A sync that keeps failing the same way is logged once, not every few minutes.
                if s.last_error.as_deref() != Some(message.as_str()) {
                    crate::applog::error("trello", &message);
                }
                s.last_error = Some(message);
            });
            Err(e)
        }
    }
}

// ---- setting up: the logic, apart from the commands ---------------------------------------------------

/// Tries the credentials: the name of the Trello account they belong to.
pub fn test_connection(trello: &dyn Trello) -> TResult<String> {
    let me = trello.me()?;
    Ok(format!("Connected to Trello as {}.", me.display()))
}

/// The key and token to use: what was typed, or else what is saved.
pub fn resolve_credentials(root: &Path, api_key: Option<&str>, token: Option<&str>) -> Result<Credentials> {
    let saved = config::read_credentials(root);
    let pick = |typed: Option<&str>, saved: String| typed.map(str::trim).filter(|t| !t.is_empty()).map_or(saved, str::to_string);
    let credentials = Credentials { api_key: pick(api_key, saved.api_key), token: pick(token, saved.token) };
    if credentials.api_key.is_empty() {
        return Err(Error::invalid("Enter the Trello API key"));
    }
    if credentials.token.is_empty() {
        return Err(Error::invalid("Enter the Trello token"));
    }
    if !valid_credential(&credentials.api_key) || !valid_credential(&credentials.token) {
        return Err(Error::invalid("The API key and token are made of letters and digits, with no spaces. Copy them again from Trello."));
    }
    Ok(credentials)
}

pub fn save_credentials(root: &Path, api_key: Option<&str>, token: Option<&str>) -> Result<()> {
    config::write_credentials(root, &resolve_credentials(root, api_key, token)?)
}

/// Removes the key and token from this machine. The person and the project links stay.
pub fn disconnect(root: &Path) -> Result<()> {
    config::write_credentials(root, &Credentials::default())
}

/// Chooses the person to sync for, who must be a member of the board they were picked from.
pub fn save_member(root: &Path, trello: &dyn Trello, board: &str, member: &str) -> TResult<Member> {
    if !trello.boards()?.iter().any(|b| b.id == board) {
        return Err(TrelloError::message("That board isn't one of your open Trello boards."));
    }
    let chosen = trello
        .board_members(board)?
        .into_iter()
        .find(|m| m.id == member)
        .ok_or_else(|| TrelloError::message("That person isn't a member of the board."))?;
    config::update_settings(root, |s| s.member = Some(chosen.clone()))?;
    Ok(chosen)
}

/// Links a project to a board (`None` unlinks it). The person must be a member of the board, and a board can serve
/// one project only: two projects reading the same cards would each import them.
pub fn link_project(root: &Path, trello: &dyn Trello, project: &str, board: Option<&str>) -> TResult<()> {
    let settings = config::read_settings(root);
    let Some(board) = board.filter(|b| !b.is_empty()) else {
        config::update_settings(root, |s| {
            s.projects.remove(project);
        })?;
        config::update_state(root, |s| s.items.retain(|_, t| t.project != project))?;
        return Ok(());
    };

    let Some(member) = settings.member else {
        return Err(TrelloError::message("Choose whose cards to sync first."));
    };
    if let Some((other, _)) = settings.projects.iter().find(|(slug, link)| link.board == board && slug.as_str() != project) {
        return Err(TrelloError::message(format!(
            "That board is already linked to the project {other}. A board can serve one project, or its cards would be added to both."
        )));
    }
    let found: Board = trello
        .boards()?
        .into_iter()
        .find(|b| b.id == board)
        .ok_or_else(|| TrelloError::message("That board isn't one of your open Trello boards."))?;
    if !trello.board_members(board)?.iter().any(|m| m.id == member.id) {
        return Err(TrelloError::message(format!(
            "{} isn't a member of the board {}. Add them to it on Trello, or choose another board.",
            member.display(),
            found.name
        )));
    }

    let changed = settings.projects.get(project).is_none_or(|link| link.board != board);
    config::update_settings(root, |s| {
        if changed {
            s.projects.insert(project.to_string(), ProjectLink { board: found.id.clone(), board_name: found.name.clone(), columns: Default::default() });
        }
    })?;
    if changed {
        // Cards of the old board are not on this one: whatever was synced stops, and says so on the next round.
        config::update_state(root, |s| s.items.retain(|_, t| t.project != project))?;
    }
    Ok(())
}

/// Says which Trello list a column uses: "auto" (the list with the same name), "none" (not synced) or a list id.
pub fn set_column_list(root: &Path, project: &str, column: &str, choice: &str) -> Result<()> {
    if !config::read_settings(root).projects.contains_key(project) {
        return Err(Error::invalid("Link the project to a Trello board first"));
    }
    config::update_settings(root, |s| {
        if let Some(link) = s.projects.get_mut(project) {
            match choice {
                "auto" => {
                    link.columns.remove(column);
                }
                "none" => {
                    link.columns.insert(column.to_string(), String::new());
                }
                list => {
                    link.columns.insert(column.to_string(), list.to_string());
                }
            }
        }
    })?;
    Ok(())
}

// ---- the views the screen gets ------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectState {
    pub slug: String,
    pub name: String,
    pub board: Option<String>,
    pub board_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnlinkedItem {
    pub project: String,
    pub project_name: String,
    pub kind: Kind,
    pub id: String,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrelloView {
    /// The key and token themselves never reach the screen: only whether they exist and how they end.
    pub has_key: bool,
    pub key_hint: String,
    pub has_token: bool,
    pub token_hint: String,
    pub member: Option<Member>,
    pub configured: bool,
    pub projects: Vec<ProjectState>,
    pub unlinked: Vec<UnlinkedItem>,
    pub status: TrelloStatus,
    pub settings_file: String,
    pub secrets_file: String,
}

pub fn view(root: &Path, snapshot: &trello_ledger::Snapshot, status: TrelloStatus) -> TrelloView {
    let settings = config::read_settings(root);
    let credentials = config::read_credentials(root);
    let mut projects = vec![];
    let mut unlinked = vec![];
    for data in &snapshot.projects {
        let link = settings.projects.get(&data.slug).filter(|l| !l.board.is_empty());
        projects.push(ProjectState {
            slug: data.slug.clone(),
            name: data.name.clone(),
            board: link.map(|l| l.board.clone()),
            board_name: link.map(|l| l.board_name.clone()),
        });
        for task in data.tasks.iter().filter(|t| t.trello_unlinked.is_some()) {
            unlinked.push(UnlinkedItem {
                project: data.slug.clone(),
                project_name: data.name.clone(),
                kind: Kind::Task,
                id: task.id.clone(),
                title: task.title.clone(),
                reason: task.trello_unlinked.clone().unwrap_or_default(),
            });
        }
        for story in data.stories.iter().filter(|s| s.trello_unlinked.is_some()) {
            unlinked.push(UnlinkedItem {
                project: data.slug.clone(),
                project_name: data.name.clone(),
                kind: Kind::Story,
                id: story.id.clone(),
                title: story.summary.clone(),
                reason: story.trello_unlinked.clone().unwrap_or_default(),
            });
        }
    }
    projects.sort_by_key(|p| p.name.to_lowercase());
    TrelloView {
        has_key: !credentials.api_key.is_empty(),
        key_hint: config::hint(&credentials.api_key),
        has_token: !credentials.token.is_empty(),
        token_hint: config::hint(&credentials.token),
        member: settings.member,
        configured: config::is_configured(root),
        projects,
        unlinked,
        status,
        settings_file: config::settings_path(root).display().to_string(),
        secrets_file: crate::settings::secrets_path(root).display().to_string(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ColumnChoice {
    pub id: String,
    pub name: String,
    /// "auto", "none" or the id of a list.
    pub choice: String,
    /// The list the column uses in the end, if it has one.
    pub list_id: Option<String>,
    pub list_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMapping {
    pub lists: Vec<List>,
    pub columns: Vec<ColumnChoice>,
    /// Lists no column uses: their cards aren't added.
    pub unused_lists: Vec<String>,
}

pub fn mapping_view(columns: &[Column], lists: &[List], link: &ProjectLink) -> ProjectMapping {
    let mapping = Mapping::new(columns, lists, &link.columns);
    let list_name = |id: &str| lists.iter().find(|l| l.id == id).map(|l| l.name.clone());
    let columns_out = columns
        .iter()
        .map(|column| {
            let list_id = mapping.list_for(&column.id).map(str::to_string);
            ColumnChoice {
                id: column.id.clone(),
                name: column.name.clone(),
                choice: match link.columns.get(&column.id).map(String::as_str) {
                    None => "auto".to_string(),
                    Some("") => "none".to_string(),
                    Some(list) => list.to_string(),
                },
                list_name: list_id.as_deref().and_then(list_name),
                list_id,
            }
        })
        .collect();
    let unused_lists = lists.iter().filter(|l| mapping.column_for(&l.id).is_none()).map(|l| l.name.clone()).collect();
    ProjectMapping { lists: lists.to_vec(), columns: columns_out, unused_lists }
}

// ---- commands -----------------------------------------------------------------------------------------

fn root_of(app: &App) -> PathBuf {
    app.lock().vault.root().to_path_buf()
}

fn current_view(app: &App, syncer: &TrelloSyncer) -> Result<TrelloView> {
    let (root, snapshot) = {
        let core = app.lock();
        (core.vault.root().to_path_buf(), trello_ledger::snapshot(&core.conn)?)
    };
    Ok(view(&root, &snapshot, syncer.status()))
}

/// Runs something that talks to Trello off the UI thread.
async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| Error::sync(format!("That stopped unexpectedly: {e}")))?
}

fn client(root: &Path) -> Result<HttpTrello<ReqwestWire>> {
    let credentials = config::read_credentials(root);
    if !credentials.complete() {
        return Err(Error::invalid("Save the Trello API key and token first"));
    }
    Ok(HttpTrello::new(&credentials.api_key, &credentials.token)?)
}

#[tauri::command]
pub fn trello_get(app: State<App>, syncer: State<TrelloSyncer>) -> Result<TrelloView> {
    current_view(&app, &syncer)
}

#[tauri::command]
pub fn trello_status(syncer: State<TrelloSyncer>) -> TrelloStatus {
    syncer.status()
}

/// Tries the key and token on screen (a blank one means the saved one) without saving anything.
#[tauri::command]
pub async fn trello_test(app: State<'_, App>, api_key: Option<String>, token: Option<String>) -> Result<String> {
    let root = root_of(&app);
    blocking(move || {
        let credentials = resolve_credentials(&root, api_key.as_deref(), token.as_deref())?;
        let client = HttpTrello::new(&credentials.api_key, &credentials.token)?;
        Ok(test_connection(&client)?)
    })
    .await
}

/// Saves the key and token (a blank one keeps the saved one).
#[tauri::command]
pub fn trello_save_credentials(app: State<App>, syncer: State<TrelloSyncer>, api_key: Option<String>, token: Option<String>) -> Result<TrelloView> {
    let root = root_of(&app);
    save_credentials(&root, api_key.as_deref(), token.as_deref())?;
    syncer.reconfigure(&root);
    current_view(&app, &syncer)
}

/// Removes the key and token from this machine.
#[tauri::command]
pub fn trello_disconnect(app: State<App>, syncer: State<TrelloSyncer>) -> Result<TrelloView> {
    let root = root_of(&app);
    disconnect(&root)?;
    syncer.reconfigure(&root);
    current_view(&app, &syncer)
}

#[tauri::command]
pub async fn trello_boards(app: State<'_, App>) -> Result<Vec<Board>> {
    let root = root_of(&app);
    blocking(move || Ok(client(&root)?.boards()?)).await
}

#[tauri::command]
pub async fn trello_board_members(app: State<'_, App>, board: String) -> Result<Vec<Member>> {
    let root = root_of(&app);
    blocking(move || Ok(client(&root)?.board_members(&board)?)).await
}

/// Chooses the person to sync for, from the members of a board.
#[tauri::command]
pub async fn trello_save_member(app: State<'_, App>, syncer: State<'_, TrelloSyncer>, board: String, member: String) -> Result<TrelloView> {
    let root = root_of(&app);
    {
        let root = root.clone();
        blocking(move || Ok(save_member(&root, &client(&root)?, &board, &member)?)).await?;
    }
    syncer.reconfigure(&root);
    current_view(&app, &syncer)
}

/// Links a project to a board, or (no board) stops syncing the project.
#[tauri::command]
pub async fn trello_link_project(app: State<'_, App>, syncer: State<'_, TrelloSyncer>, project: String, board: Option<String>) -> Result<TrelloView> {
    let root = root_of(&app);
    app.read(|c, _| projects::require(c, &project))?;
    {
        let root = root.clone();
        blocking(move || Ok(link_project(&root, &client(&root)?, &project, board.as_deref())?)).await?;
    }
    syncer.reconfigure(&root);
    current_view(&app, &syncer)
}

/// The project's columns, the board's lists, and which goes with which.
#[tauri::command]
pub async fn trello_project_mapping(app: State<'_, App>, project: String) -> Result<ProjectMapping> {
    let root = root_of(&app);
    let columns = app.read(|c, _| crate::board::columns(c, &project))?;
    let link = config::read_settings(&root).projects.get(&project).cloned().filter(|l| !l.board.is_empty());
    let Some(link) = link else { return Err(Error::invalid("This project isn't linked to a Trello board")) };
    blocking(move || {
        let lists = client(&root)?.lists(&link.board)?;
        Ok(mapping_view(&columns, &lists, &link))
    })
    .await
}

/// "auto", "none" or a list id for one column.
#[tauri::command]
pub fn trello_set_column_list(app: State<App>, syncer: State<TrelloSyncer>, project: String, column: String, choice: String) -> Result<()> {
    if !app.read(|c, _| crate::board::column_exists(c, &project, &column))? {
        return Err(Error::not_found("Column not found"));
    }
    let root = root_of(&app);
    set_column_list(&root, &project, &column, &choice)?;
    syncer.reconfigure(&root);
    Ok(())
}

#[tauri::command]
pub fn trello_sync_now(app: State<App>, syncer: State<TrelloSyncer>) -> Result<()> {
    if !config::is_configured(&root_of(&app)) {
        return Err(Error::invalid("Trello sync isn't set up yet: it needs the key and token, a person, and a project linked to a board"));
    }
    syncer.now();
    Ok(())
}

/// Lets an item that stopped syncing get a card of its own again: the old card is left alone on Trello, and the next
/// sync makes a new one.
#[tauri::command]
pub fn trello_relink_item(app: State<App>, syncer: State<TrelloSyncer>, project: String, kind: Kind, id: String) -> Result<TrelloView> {
    app.mutate(|conn, root| {
        if let Some(card) = trello_ledger::clear_link(conn, &project, kind, &id)? {
            config::update_state(root, |s| {
                s.items.remove(&card);
                s.released.insert(card);
            })?;
        }
        Ok(())
    })?;
    syncer.now();
    current_view(&app, &syncer)
}
