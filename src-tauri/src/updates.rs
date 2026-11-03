//! Updating Ledger from inside the app: looking for a new release, showing what changed, and installing it once a
//! restart would cut nothing off.
//!
//! * **Release builds only.** The updater plugin is registered in release builds alone (`lib.rs`), so `tauri dev`,
//!   debug builds and sandboxes never ask anyone for an update. A debug build can simulate one instead
//!   ([`simulate_update`]): the notice shows, and installing goes through every step but installs nothing.
//! * **Where from.** `plugins.updater` in `tauri.conf.json`: the `latest.json` of the newest published GitHub release,
//!   and the public key the installers must be signed with. The release workflow publishes a release only once the
//!   installers and signatures of every platform are on it. A build without a real key never looks.
//! * **When.** Shortly after starting and every few hours, unless that is turned off under Settings → About (a choice
//!   for this machine, kept in the app's `config.json`; see `settings.rs`), and never while a sync runs. A check that
//!   fails (offline, or no release with installers yet) says nothing, except when asked for under Settings.
//! * **Installing** restarts Ledger, so it waits until that cuts nothing off: a GitHub or Trello sync round finishes,
//!   and Claude Code runs end, or are stopped if the person says so ([`readiness`]). The last look is taken on the
//!   main thread with the vault locked, and the restart happens right there, so nothing can start or be written in
//!   between. Every change is in the vault's files by then (`App::mutate` writes through), and what isn't on GitHub
//!   yet is pushed by the sync Ledger starts with. The signature is checked before anything is installed: an update
//!   that doesn't match the key is refused, and the notice says why.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::error::{Error, Result};
use crate::state::App;
use crate::trello::TrelloSyncer;
use crate::{claude_code, ids, markdown, settings};

/// Whether this build has the updater at all: release builds do, debug builds (`tauri dev`, the sandbox) don't.
/// `lib.rs` registers the plugin on the same condition.
pub const BUILT_IN: bool = cfg!(not(debug_assertions));

/// How soon after starting the first look is taken (the window and the first sync go first), how often after that,
/// and how long to wait when a sync is running at the time.
pub const FIRST_CHECK: Duration = Duration::from_secs(20);
pub const EVERY: Duration = Duration::from_secs(4 * 60 * 60);
pub const SYNC_WAIT: Duration = Duration::from_secs(60);
/// The scheduler wakes this often to see whether a check is due: by the clock, so a computer that slept still checks.
const TICK: Duration = Duration::from_secs(60);

/// Told to the window: an update was found (`UpdateInfo`), and how far its download has got (`Progress`).
pub const EVENT_AVAILABLE: &str = "update-available";
pub const EVENT_PROGRESS: &str = "update-progress";

const NO_KEY: &str = "This build has no updater key yet, so it doesn't look for updates. Install a release of Ledger to get them.";
const DEV_BUILD: &str = "This is a development build: it never looks for updates.";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- whether it is safe to restart: the decision, apart from Tauri so it can be tested ----------------------

/// What is going on that a restart would cut off.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// A GitHub sync round is running.
    pub github_syncing: bool,
    /// Changes made here are not on GitHub yet. Not a reason to wait: they are in the vault's files, and the sync
    /// Ledger starts with pushes them.
    pub github_pending: bool,
    /// A Trello sync round is running.
    pub trello_syncing: bool,
    /// The prompts Claude Code is still running, by title. Restarting would stop them.
    pub runs: Vec<String>,
}

/// What to do about Claude Code runs that are still going.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunsChoice {
    /// Nobody has said yet: ask.
    #[default]
    Ask,
    /// Wait until they finish by themselves.
    Wait,
    /// Stop them, and install once they have stopped.
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Readiness {
    /// Nothing would be cut off: install now. The notes are worth knowing but change nothing.
    Ready { notes: Vec<String> },
    /// Something that ends by itself is still going: look again in a moment.
    Wait { waiting_for: Vec<String> },
    /// Claude Code is running these prompts: the person decides whether to wait for them or stop them.
    Ask { runs: Vec<String> },
}

/// Whether Ledger can restart now. A sync round is always waited for: it ends by itself, and cutting it off halfway
/// would only make the next one redo it. Claude Code runs can take long and use the person's allowance, so they decide.
pub fn readiness(activity: &Activity, choice: RunsChoice) -> Readiness {
    if !activity.runs.is_empty() && choice == RunsChoice::Ask {
        return Readiness::Ask { runs: activity.runs.clone() };
    }
    let mut waiting_for = Vec::new();
    if activity.github_syncing {
        waiting_for.push("the GitHub sync to finish".to_string());
    }
    if activity.trello_syncing {
        waiting_for.push("the Trello sync to finish".to_string());
    }
    if !activity.runs.is_empty() {
        let prompts = quoted(&activity.runs);
        waiting_for.push(match choice {
            RunsChoice::Stop => format!("Claude Code to stop {prompts}"),
            _ => format!("Claude Code to finish {prompts}"),
        });
    }
    if !waiting_for.is_empty() {
        return Readiness::Wait { waiting_for };
    }
    let mut notes = Vec::new();
    if activity.github_pending {
        notes.push("Your latest changes aren't on GitHub yet. They are saved in the vault, and Ledger pushes them when it starts again.".to_string());
    }
    Readiness::Ready { notes }
}

fn quoted(titles: &[String]) -> String {
    let titles: Vec<String> = titles.iter().map(|t| format!("“{}”", if t.trim().is_empty() { "a prompt" } else { t.trim() })).collect();
    match titles.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What the scheduler does when a check is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    Check,
    /// Automatic checks are off: look again at the next turn, in case they are switched back on.
    Skip,
    /// A sync is running: try again shortly.
    AfterSync,
}

pub fn when_due(automatic: bool, syncing: bool) -> Due {
    match (automatic, syncing) {
        (false, _) => Due::Skip,
        (true, true) => Due::AfterSync,
        (true, false) => Due::Check,
    }
}

// ---- what the notice shows ---------------------------------------------------------------------------------

/// Whether going from `current` to `next` is a major release: the first number goes up. The release script makes a
/// breaking change exactly that (0.4.2 → 1.0.0).
pub fn is_major(current: &str, next: &str) -> bool {
    fn major(version: &str) -> Option<u64> {
        version.trim().trim_start_matches('v').split(['.', '-', '+']).next()?.parse().ok()
    }
    matches!((major(current), major(next)), (Some(from), Some(to)) if to > from)
}

/// Release notes, with the "Breaking changes" section (the release script writes it first) taken apart, so the
/// notice can make it stand out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// The section's content, without its heading.
    pub breaking: Option<String>,
    pub rest: String,
}

pub fn split_notes(notes: &str) -> Notes {
    let heading = |line: &str| -> Option<(usize, String)> {
        let trimmed = line.trim_start();
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        let text = trimmed[level..].strip_prefix(' ')?;
        (1..=6).contains(&level).then(|| (level, text.trim().trim_end_matches('#').trim().to_lowercase()))
    };

    let (mut breaking, mut rest) = (Vec::new(), Vec::new());
    let mut inside: Option<usize> = None;
    let mut fenced = false;
    for line in notes.lines() {
        let fence = line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~");
        if !fenced && let Some((level, text)) = heading(line) {
            if inside.is_some_and(|at| level <= at) {
                inside = None;
            }
            if inside.is_none() && text == "breaking changes" {
                inside = Some(level);
                continue;
            }
        }
        if fence {
            fenced = !fenced;
        }
        if inside.is_some() { breaking.push(line) } else { rest.push(line) }
    }
    let breaking = breaking.join("\n").trim().to_string();
    Notes { breaking: (!breaking.is_empty()).then_some(breaking), rest: rest.join("\n").trim().to_string() }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub current_version: String,
    /// When the release was published, as ISO text, if the release says.
    pub date: Option<String>,
    /// The first number goes up: something may work differently, and the notice says so.
    pub major: bool,
    /// The release notes as HTML, without the breaking changes.
    pub notes_html: String,
    /// The "Breaking changes" section as HTML, shown apart.
    pub breaking_html: Option<String>,
    /// A development build's pretend update: installing it goes through every step but installs nothing.
    pub simulated: bool,
}

pub fn update_info(version: &str, current_version: &str, date: Option<&str>, notes: Option<&str>, simulated: bool) -> UpdateInfo {
    let notes = split_notes(notes.unwrap_or_default());
    UpdateInfo {
        version: version.trim_start_matches('v').to_string(),
        current_version: current_version.trim_start_matches('v').to_string(),
        date: date.map(str::to_string).filter(|d| !d.trim().is_empty()),
        major: is_major(current_version, version),
        notes_html: markdown::render(&notes.rest),
        breaking_html: notes.breaking.as_deref().map(markdown::render),
        simulated,
    }
}

/// Why this build can't look for updates with the public key it was built with, if it can't: no key, or the
/// placeholder that stands in for it until the real one is generated. A real one is a minisign public key in base64.
pub fn key_problem(pubkey: Option<&str>) -> Option<&'static str> {
    let decoded = pubkey
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .and_then(|key| STANDARD.decode(key).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    match decoded {
        Some(text) if text.contains("minisign public key") => None,
        _ => Some(NO_KEY),
    }
}

/// A failed check, in words for Settings (an automatic check says nothing).
fn check_failure(e: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    match e {
        E::ReleaseNotFound => "No update information was found: GitHub couldn't be reached, or no release with installers has been published yet.".to_string(),
        E::Reqwest(e) => format!("GitHub couldn't be reached to look for updates: {e}"),
        E::TargetNotFound(_) | E::TargetsNotFound(_) => "The newest release has no installer for this system.".to_string(),
        other => format!("Looking for updates didn't work: {other}"),
    }
}

/// Whether the update was refused for its signature: it isn't signed with the key this build trusts, or it was
/// signed for another version than the release announced.
pub fn is_signature_error(e: &tauri_plugin_updater::Error) -> bool {
    use tauri_plugin_updater::Error as E;
    matches!(e, E::Minisign(_) | E::Base64(_) | E::SignatureUtf8(_) | E::SignedVersionMismatch { .. } | E::MissingSignedVersion)
}

/// A failed download or install, in words for the notice.
pub fn install_failure(e: &tauri_plugin_updater::Error) -> String {
    use tauri_plugin_updater::Error as E;
    if matches!(e, E::SignedVersionMismatch { .. } | E::MissingSignedVersion) {
        return format!(
            "The update was refused because it isn't signed for the version the release announces, so it may be an older release passed off as a new one. Nothing was installed. ({e})"
        );
    }
    if is_signature_error(e) {
        return format!(
            "The update was refused because its signature doesn't match Ledger's key: it was changed, or it doesn't come from Ledger's releases. Nothing was installed. ({e})"
        );
    }
    match e {
        E::Reqwest(e) => format!("The update couldn't be downloaded: {e}. Nothing was installed."),
        E::Network(e) => format!("The update couldn't be downloaded ({e}). Nothing was installed."),
        other => format!("The update couldn't be installed: {other}"),
    }
}

// ---- the running app -----------------------------------------------------------------------------------------

enum Found {
    /// What the last check found, and its download once it has been downloaded and its signature checked.
    Real { update: Box<tauri_plugin_updater::Update>, info: UpdateInfo, bytes: Option<Vec<u8>> },
    /// A development build's pretend update.
    Simulated { info: UpdateInfo, downloaded: bool },
}

impl Found {
    fn info(&self) -> &UpdateInfo {
        match self {
            Found::Real { info, .. } | Found::Simulated { info, .. } => info,
        }
    }
}

pub struct Updates {
    config_dir: PathBuf,
    found: Mutex<Option<Found>>,
    /// When a check last worked, as ISO text.
    checked_at: Mutex<Option<String>>,
    downloading: AtomicBool,
}

/// Sets up the state, and in a release build that can check, the scheduler.
pub fn start(app: &AppHandle, config_dir: PathBuf) {
    app.manage(Updates { config_dir: config_dir.clone(), found: Mutex::new(None), checked_at: Mutex::new(None), downloading: AtomicBool::new(false) });
    if unavailable(app).is_some() {
        return;
    }
    let app = app.clone();
    let _ = thread::Builder::new().name("update-check".into()).spawn(move || scheduler(app, config_dir));
}

/// Why this build doesn't look for updates, if it doesn't.
fn unavailable(app: &AppHandle) -> Option<&'static str> {
    if !BUILT_IN {
        return Some(DEV_BUILD);
    }
    let pubkey = app.config().plugins.0.get("updater").and_then(|c| c.get("pubkey")).and_then(|k| k.as_str());
    key_problem(pubkey)
}

fn scheduler(app: AppHandle, config_dir: PathBuf) {
    let mut due = SystemTime::now() + FIRST_CHECK;
    loop {
        let left = due.duration_since(SystemTime::now()).unwrap_or_default();
        if !left.is_zero() {
            thread::sleep(left.min(TICK));
            continue;
        }
        let automatic = settings::load_config(&config_dir).updates.automatic;
        let activity = activity(&app);
        due = SystemTime::now()
            + match when_due(automatic, activity.github_syncing || activity.trello_syncing) {
                Due::Skip => EVERY,
                Due::AfterSync => SYNC_WAIT,
                Due::Check => {
                    // Quietly: offline, or no release with installers yet, is nothing to bother anyone with.
                    let _ = tauri::async_runtime::block_on(check(&app));
                    EVERY
                }
            };
    }
}

/// What a restart would cut off right now. Takes the locks of the sync workers and the runs, never the vault's.
fn activity(app: &AppHandle) -> Activity {
    let github = app.state::<App>().syncer.status();
    let trello_syncing = app.try_state::<TrelloSyncer>().is_some_and(|t| t.status().state == "syncing");
    Activity { github_syncing: github.state == "syncing", github_pending: github.pending, trello_syncing, runs: claude_code::running(app) }
}

/// Asks the release server whether there is a newer version, and tells the window when there is.
async fn check(app: &AppHandle) -> Result<Option<UpdateInfo>> {
    if let Some(reason) = unavailable(app) {
        return Err(Error::update(reason));
    }
    let updater = app.updater().map_err(|e| Error::update(format!("The updater couldn't start: {e}")))?;
    let found = updater.check().await.map_err(|e| Error::update(check_failure(&e)))?;
    let updates = app.state::<Updates>();
    *lock(&updates.checked_at) = Some(ids::now());
    let Some(update) = found else {
        *lock(&updates.found) = None;
        return Ok(None);
    };
    let date = update.raw_json.get("pub_date").and_then(|d| d.as_str());
    let info = update_info(&update.version, &update.current_version, date, update.body.as_deref(), false);
    {
        let mut found = lock(&updates.found);
        // The same release again keeps what was already downloaded; a release that changed (another file, another
        // signature) starts over.
        let same = matches!(&*found, Some(Found::Real { update: old, .. })
            if old.version == update.version && old.download_url == update.download_url && old.signature == update.signature);
        if !same {
            *found = Some(Found::Real { update: Box::new(update), info: info.clone(), bytes: None });
        }
    }
    let _ = app.emit(EVENT_AVAILABLE, &info);
    Ok(Some(info))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    downloaded: u64,
    total: Option<u64>,
}

/// Downloads the update and checks its signature, once.
async fn ensure_downloaded(app: &AppHandle) -> Result<()> {
    let updates = app.state::<Updates>();
    let update = match &*lock(&updates.found) {
        None => return Err(Error::update("There is no update to install. Look for updates under Settings → About.")),
        Some(Found::Real { bytes: Some(_), .. } | Found::Simulated { downloaded: true, .. }) => return Ok(()),
        Some(Found::Real { update, .. }) => Some((**update).clone()),
        Some(Found::Simulated { .. }) => None,
    };
    if updates.downloading.swap(true, Ordering::SeqCst) {
        return Err(Error::update("The update is already being downloaded"));
    }
    let result = match &update {
        Some(update) => download(app, update).await,
        None => simulate_download(app).await,
    };
    updates.downloading.store(false, Ordering::SeqCst);
    let bytes = result?;
    match (&mut *lock(&updates.found), update) {
        (Some(Found::Real { info, bytes: slot, .. }), Some(update)) if info.version == update.version => *slot = Some(bytes),
        (Some(Found::Simulated { downloaded, .. }), None) => *downloaded = true,
        // A newer check replaced the update meanwhile: the next try downloads that one.
        _ => return Err(Error::update("A newer update was found meanwhile. Try again to install that one.")),
    }
    Ok(())
}

async fn download(app: &AppHandle, update: &tauri_plugin_updater::Update) -> Result<Vec<u8>> {
    let emitter = app.clone();
    let mut downloaded: u64 = 0;
    let mut told: Option<Instant> = None;
    update
        .download(
            move |chunk, total| {
                downloaded += chunk as u64;
                if told.is_none_or(|at| at.elapsed() >= Duration::from_millis(100)) || total == Some(downloaded) {
                    told = Some(Instant::now());
                    let _ = emitter.emit(EVENT_PROGRESS, Progress { downloaded, total });
                }
            },
            || {},
        )
        .await
        .map_err(|e| Error::update(install_failure(&e)))
}

/// A pretend download, for a development build: the progress the window shows, over about a second and a half.
async fn simulate_download(app: &AppHandle) -> Result<Vec<u8>> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let total = 24 * 1024 * 1024;
        for step in 1..=12u64 {
            thread::sleep(Duration::from_millis(120));
            let _ = app.emit(EVENT_PROGRESS, Progress { downloaded: total * step / 12, total: Some(total) });
        }
    })
    .await
    .map_err(|e| Error::update(format!("The download stopped unexpectedly: {e}")))?;
    Ok(Vec::new())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum InstallStep {
    /// Not yet: look again in a moment.
    Wait { waiting_for: Vec<String> },
    /// Claude Code is running these prompts: wait for them, or stop them?
    Ask { runs: Vec<String> },
    /// A development build's pretend update went through every step; a real one would have restarted here.
    Simulated { version: String },
}

/// The step to report when Ledger can't restart yet; `None` when it can.
fn not_yet(readiness: Readiness) -> Option<InstallStep> {
    match readiness {
        Readiness::Ready { .. } => None,
        Readiness::Wait { waiting_for } => Some(InstallStep::Wait { waiting_for }),
        Readiness::Ask { runs } => Some(InstallStep::Ask { runs }),
    }
}

/// The last look and the install, on the main thread. The vault stays locked from here on, so a change being written
/// finishes first and nothing is written after; no Claude Code run can start either. A real update ends with the
/// restart: on Windows the installer has already taken over (and starts Ledger again when it is done), on macOS and
/// Linux the new version is in place and is started here.
fn install_on_main_thread(app: &AppHandle, choice: RunsChoice) -> Result<InstallStep> {
    let state = app.state::<App>();
    let _vault = state.lock();
    let activity = activity(app);
    if let Some(step) = not_yet(readiness(&activity, choice)) {
        return Ok(step);
    }
    if let Err(runs) = claude_code::hold_for_restart(app) {
        // A run started after the look above: as if it had been there all along.
        let step = not_yet(readiness(&Activity { runs, ..activity }, choice));
        return Ok(step.unwrap_or(InstallStep::Wait { waiting_for: Vec::new() }));
    }

    let updates = app.state::<Updates>();
    let found = lock(&updates.found);
    let installed = match &*found {
        Some(Found::Real { update, bytes: Some(bytes), .. }) => update.install(bytes).map_err(|e| Error::update(install_failure(&e))),
        Some(Found::Simulated { info, .. }) => {
            claude_code::release_hold(app);
            return Ok(InstallStep::Simulated { version: info.version.clone() });
        }
        _ => Err(Error::update("The update isn't downloaded yet. Try again.")),
    };
    if let Err(e) = installed {
        claude_code::release_hold(app);
        return Err(e);
    }
    app.restart()
}

// ---- commands -----------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateView {
    pub current_version: String,
    /// Look by itself shortly after starting and every few hours.
    pub automatic: bool,
    /// Why this build doesn't look for updates, if it doesn't.
    pub unavailable: Option<String>,
    /// A development build, which can simulate an update.
    pub dev_build: bool,
    pub available: Option<UpdateInfo>,
    pub checked_at: Option<String>,
    /// Where the choice is kept.
    pub config_file: String,
}

fn view(app: &AppHandle) -> UpdateView {
    let updates = app.state::<Updates>();
    UpdateView {
        current_version: app.package_info().version.to_string(),
        automatic: settings::load_config(&updates.config_dir).updates.automatic,
        unavailable: unavailable(app).map(str::to_string),
        dev_build: !BUILT_IN,
        available: lock(&updates.found).as_ref().map(|f| f.info().clone()),
        checked_at: lock(&updates.checked_at).clone(),
        config_file: updates.config_dir.join("config.json").display().to_string(),
    }
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> UpdateView {
    view(&app)
}

#[tauri::command]
pub fn save_update_settings(app: AppHandle, automatic: bool) -> Result<UpdateView> {
    settings::update_config(&app.state::<Updates>().config_dir, |config| config.updates.automatic = automatic)?;
    Ok(view(&app))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub available: Option<UpdateInfo>,
    pub message: String,
}

/// Looks for an update now, whether or not automatic checks are on; what went wrong is said.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<CheckResult> {
    if !BUILT_IN {
        let available = lock(&app.state::<Updates>().found).as_ref().map(|f| f.info().clone());
        return Ok(CheckResult { available, message: format!("{DEV_BUILD} Simulate one below to see how an update looks.") });
    }
    let available = check(&app).await?;
    let message = match &available {
        Some(info) => format!("Ledger {} is available.", info.version),
        None => format!("You have the newest version, Ledger {}.", app.package_info().version),
    };
    Ok(CheckResult { available, message })
}

/// Installs the update and restarts, once that cuts nothing off. Called again while the answer is to wait; the
/// download happens once. With `Stop`, the Claude Code runs still going are stopped first.
#[tauri::command]
pub async fn install_update(app: AppHandle, choice: RunsChoice) -> Result<InstallStep> {
    if choice == RunsChoice::Stop {
        claude_code::stop_all(&app);
    }
    // Ask before downloading anything: the answer may well be "not now".
    if let Readiness::Ask { runs } = readiness(&activity(&app), choice) {
        return Ok(InstallStep::Ask { runs });
    }
    ensure_downloaded(&app).await?;
    if let Some(step) = not_yet(readiness(&activity(&app), choice)) {
        return Ok(step);
    }

    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(install_on_main_thread(&handle, choice));
    })
    .map_err(|e| Error::update(format!("The update couldn't be started: {e}")))?;
    tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|e| Error::update(format!("The update stopped unexpectedly: {e}")))?
        .map_err(|_| Error::update("The update stopped unexpectedly"))?
}

/// The next version a simulated update pretends to be.
pub fn simulated_version(current: &str, major: bool) -> String {
    let mut parts = current.trim_start_matches('v').split(['-', '+']).next().unwrap_or_default().split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    let (x, y) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if major { format!("{}.0.0", x + 1) } else { format!("{x}.{}.0", y + 1) }
}

/// Release notes for a simulated update, written like the release script writes them.
pub fn simulated_notes(major: bool) -> String {
    let breaking = "### Breaking changes\n\n- **sync:** re-enter your GitHub token under Settings → Sync to GitHub: it is now kept per vault ([a1b2c3d](https://github.com/openledger-nl/ledger/commits))\n\n";
    format!(
        "{}### Features\n\n- **board:** drag cards between columns ([d4e5f6a](https://github.com/openledger-nl/ledger/commits))\n- **updates:** install new releases from inside the app ([b7c8d9e](https://github.com/openledger-nl/ledger/commits))\n\n### Fixes\n\n- **search:** find words with accents when typed without them ([f0a1b2c](https://github.com/openledger-nl/ledger/commits))",
        if major { breaking } else { "" }
    )
}

/// Development builds only: pretend a release was found, so the notice and installing can be tried in a sandbox.
#[tauri::command]
pub fn simulate_update(app: AppHandle, major: bool) -> Result<UpdateInfo> {
    if BUILT_IN {
        return Err(Error::update("Only a development build can simulate an update"));
    }
    let current = app.package_info().version.to_string();
    let notes = simulated_notes(major);
    let info = update_info(&simulated_version(&current, major), &current, Some(&ids::now()), Some(&notes), true);
    *lock(&app.state::<Updates>().found) = Some(Found::Simulated { info: info.clone(), downloaded: false });
    let _ = app.emit(EVENT_AVAILABLE, &info);
    Ok(info)
}
