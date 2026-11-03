//! Running a prompt with Claude Code: one click in the app starts the `claude` command-line program,
//! non-interactively, in the prompt's project folder, and the deliverable it writes lands where the app expects it.
//!
//! * `config`: the settings (program path, permission mode, model, guard rails), in `.ledger/claude-code.json`.
//! * `cli`: finding the program, the command line, the guard rails' settings file.
//! * `prompt`: what is sent, and checking that the prompt and its project are there.
//! * `runner`: the traits that start a process, and the real implementation (no shell, no console window on
//!   Windows, the whole process tree stopped on cancel).
//! * `runs`: the runs themselves, with their output, cancel, and tidying the vault afterwards.
//! * `versions`: which deliverables a run changed, and what is published and linked to the prompt for them.
//! * `render`, `redact`, `records`, `finish`: showing the output, keeping secrets out of it, the record of a run
//!   that is kept in its thread as a note, and the vault reload.
//!
//! This file holds the Tauri side: the commands, and the host that tells the window what is happening. The runs live
//! in a state of their own that is set up the first time it is needed, so the app's own setup is untouched.

pub mod cli;
pub mod config;
pub mod finish;
pub mod prompt;
pub mod records;
pub mod redact;
pub mod render;
pub mod runner;
pub mod runs;
pub mod versions;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, Window, WindowEvent};

use self::cli::Search;
use self::config::ClaudeSettings;
use self::prompt::RunRequest;
use self::records::RunRecord;
use self::runner::{OsRunner, Runner, SpawnSpec};
use self::runs::{Host, RunContext, RunEvent, RunSnapshot, RunView, Runs, VaultOutcome};
use self::versions::Change;
use crate::error::{Error, Result};
use crate::state::App;

// ---- the host ---------------------------------------------------------------------------------------

/// Tells the window what happens to a run, and reads the vault in again when one is over.
struct TauriHost {
    app: AppHandle,
}

impl Host for TauriHost {
    fn emit(&self, event: &RunEvent) {
        let _ = self.app.emit("claude-run", event);
    }

    fn vault_after_run(&self, request: &RunRequest, changes: &[Change], link: bool) -> VaultOutcome {
        let state = self.app.state::<App>();
        let outcome = {
            let mut core = state.lock();
            finish::refresh_vault(&mut core, request, changes, link)
        };
        if outcome.reloaded || !outcome.linked.is_empty() || !outcome.kept.is_empty() {
            // What the run wrote is part of the vault now, and goes to GitHub like anything else; the window refreshes.
            state.syncer.dirty();
            let _ = self.app.emit("vault-changed", ());
        }
        outcome
    }

    fn keep_record(&self, record: &RunRecord) -> std::result::Result<(), String> {
        let state = self.app.state::<App>();
        {
            let mut core = state.lock();
            finish::keep_record(&mut core, record).map_err(|e| e.to_string())?;
        }
        // The note is part of the vault now: it goes to GitHub like anything else, and the window refreshes.
        state.syncer.dirty();
        let _ = self.app.emit("vault-changed", ());
        Ok(())
    }
}

/// The runs, set up the first time they are asked for.
fn manager(app: &AppHandle) -> Runs {
    if let Some(runs) = app.try_state::<Runs>() {
        return runs.inner().clone();
    }
    app.manage(Runs::new(Arc::new(OsRunner), Arc::new(TauriHost { app: app.clone() })));
    app.state::<Runs>().inner().clone()
}

/// Closing the main window stops the runs still going: Claude Code can edit files and uses your Claude allowance, and with the
/// window gone there would be no way to see or stop it. Registered with `Builder::on_window_event`.
pub fn on_window_event(window: &Window, event: &WindowEvent) {
    if matches!(event, WindowEvent::Destroyed)
        && window.label() == "main"
        && let Some(runs) = window.app_handle().try_state::<Runs>()
    {
        runs.cancel_all();
    }
}

fn vault_root(app: &App) -> PathBuf {
    app.lock().vault.root().to_path_buf()
}

// ---- restarting to install an update (see `updates.rs`) ---------------------------------------------

/// The prompts Claude Code is running now, by title.
pub fn running(app: &AppHandle) -> Vec<String> {
    app.try_state::<Runs>().map(|runs| runs.running().into_iter().map(|r| r.prompt_title).collect()).unwrap_or_default()
}

/// Stops every run still going, each once: asking again while one is being stopped changes nothing.
pub fn stop_all(app: &AppHandle) {
    if let Some(runs) = app.try_state::<Runs>() {
        for run in runs.running() {
            let _ = runs.cancel(&run.id);
        }
    }
}

/// Before the restart: succeeds when no run is going, and none can start after. Otherwise the prompts still running.
pub fn hold_for_restart(app: &AppHandle) -> std::result::Result<(), Vec<String>> {
    manager(app).hold_for_restart().map_err(|going| going.into_iter().map(|r| r.prompt_title).collect())
}

/// The restart didn't happen: runs can start again.
pub fn release_hold(app: &AppHandle) {
    manager(app).release_hold();
}

// ---- settings and the check -------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettingsView {
    pub cli_path: String,
    pub permission_mode: String,
    pub model: String,
    pub confine: bool,
    /// A prompt that is created starts running at once.
    pub auto_run: bool,
    /// The program that would be run, when it can be found.
    pub resolved_path: Option<String>,
    /// Why it can't be found, in words for the screen.
    pub problem: Option<String>,
    pub settings_file: String,
    pub max_concurrent: usize,
}

fn settings_view(root: &Path) -> ClaudeSettingsView {
    let settings = config::load(root);
    let (resolved_path, problem) = match cli::resolve(&settings.cli_path, &Search::host()) {
        Ok(path) => (Some(path.display().to_string()), None),
        Err(e) => (None, Some(e.to_string())),
    };
    ClaudeSettingsView {
        cli_path: settings.cli_path,
        permission_mode: settings.permission_mode.as_str().to_string(),
        model: settings.model,
        confine: settings.confine,
        auto_run: settings.auto_run,
        resolved_path,
        problem,
        settings_file: config::settings_path(root).display().to_string(),
        max_concurrent: runs::MAX_CONCURRENT,
    }
}

#[tauri::command]
pub fn claude_code_settings(app: State<App>) -> ClaudeSettingsView {
    settings_view(&vault_root(&app))
}

#[tauri::command]
pub fn save_claude_code_settings(
    app: State<App>,
    cli_path: String,
    permission_mode: String,
    model: String,
    confine: bool,
    auto_run: bool,
) -> Result<ClaudeSettingsView> {
    let root = vault_root(&app);
    config::save(&root, &ClaudeSettings::validated(&cli_path, &permission_mode, &model, confine, auto_run)?)?;
    Ok(settings_view(&root))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeCheck {
    pub ok: bool,
    /// What `claude --version` printed.
    pub version: String,
    pub path: Option<String>,
    pub message: String,
}

/// Runs `claude --version`, with the path as typed on screen (blank: look for it on the PATH).
pub fn check_with(runner: &dyn Runner, cli_path: &str, search: &Search, cwd: &Path, timeout: Duration) -> ClaudeCheck {
    let fail = |path: Option<String>, message: String| ClaudeCheck { ok: false, version: String::new(), path, message };
    let program = match cli::resolve(cli_path, search) {
        Ok(program) => program,
        Err(e) => return fail(None, e.to_string()),
    };
    let shown = program.display().to_string();
    let spec = SpawnSpec { env: runs::child_env(&program), program, args: cli::version_args(), cwd: cwd.to_path_buf(), stdin: Vec::new() };
    let first_line = |text: &str| text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string();
    match runner::capture(runner, &spec, timeout) {
        Err(e) => fail(Some(shown.clone()), format!("{shown} couldn't be started: {e}")),
        Ok(out) if out.timed_out => fail(Some(shown), format!("claude --version didn't answer within {} seconds", timeout.as_secs())),
        Ok(out) if out.exit.success() => {
            let version = first_line(&out.stdout);
            ClaudeCheck { ok: true, message: format!("Found Claude Code: {version}"), version, path: Some(shown) }
        }
        Ok(out) => {
            let detail = first_line(if out.stderr.trim().is_empty() { &out.stdout } else { &out.stderr });
            fail(Some(shown.clone()), format!("{shown} stopped with {}{}", out.exit.describe(), if detail.is_empty() { String::new() } else { format!(": {detail}") }))
        }
    }
}

/// Runs `claude --version` to see that the program works.
#[tauri::command]
pub async fn check_claude_code(app: State<'_, App>, cli_path: String) -> Result<ClaudeCheck> {
    let root = vault_root(&app);
    tauri::async_runtime::spawn_blocking(move || check_with(&OsRunner, &cli_path, &Search::host(), &root, Duration::from_secs(15)))
        .await
        .map_err(|e| Error::invalid(format!("The check stopped unexpectedly: {e}")))
}

// ---- running a prompt -------------------------------------------------------------------------------

/// Everything the confirmation shows before a run starts.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPreview {
    /// Where it runs.
    pub folder: String,
    /// The command line, for reading: it is never run through a shell.
    pub command: String,
    /// The text sent on standard input, exactly.
    pub prompt: String,
    /// The settings file given with `--settings`, when the guard rails are on.
    pub guard: Option<String>,
    pub permission_mode: String,
    pub model: String,
    /// Set when it can't be run, and why.
    pub problem: Option<String>,
}

fn prepare_request(app: &AppHandle, project: &str, thread: &str, id: &str) -> Result<(PathBuf, RunRequest)> {
    let state = app.state::<App>();
    state.read(|conn, root| Ok((root.to_path_buf(), prompt::prepare(conn, root, project, thread, id)?)))
}

#[tauri::command]
pub fn claude_code_preview(app: AppHandle, project: String, thread: String, id: String) -> Result<RunPreview> {
    let (root, request) = prepare_request(&app, &project, &thread, &id)?;
    let settings = config::load(&root);
    let guard_file = config::guard_path(&root);
    let (program, problem) = match cli::resolve(&settings.cli_path, &Search::host()) {
        Ok(program) => (program, None),
        Err(e) => (PathBuf::from("claude"), Some(e.to_string())),
    };
    let args = cli::run_args(&settings, settings.confine.then_some(guard_file.as_path()));
    Ok(RunPreview {
        folder: request.cwd.display().to_string(),
        command: cli::display_command(&program, &args),
        prompt: request.text,
        guard: settings.confine.then(|| cli::guard_settings(&root)),
        permission_mode: settings.permission_mode.as_str().to_string(),
        model: settings.model,
        problem,
    })
}

/// Starts the run. `confirmed` is the text the person was shown: if the prompt has changed since, nothing starts.
#[tauri::command]
pub fn claude_code_start(app: AppHandle, project: String, thread: String, id: String, confirmed: String) -> Result<RunView> {
    let (root, request) = prepare_request(&app, &project, &thread, &id)?;
    prompt::ensure_confirmed(&request, &confirmed)?;
    let settings = config::load(&root);
    manager(&app).start(request, RunContext { root, settings }, &Search::host())
}

#[tauri::command]
pub fn claude_code_cancel(app: AppHandle, run_id: String) -> Result<RunView> {
    manager(&app).cancel(&run_id)
}

/// Every run still listed, with its output so far.
#[tauri::command]
pub fn claude_code_runs(app: AppHandle) -> Vec<RunSnapshot> {
    manager(&app).list()
}

#[tauri::command]
pub fn claude_code_run(app: AppHandle, run_id: String) -> Result<RunSnapshot> {
    manager(&app).snapshot(&run_id)
}
