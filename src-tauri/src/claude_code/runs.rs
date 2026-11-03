//! The runs themselves: starting one, following it while it works, stopping it, and putting the vault in order when
//! it is over. Each run has a thread of its own, so a run goes on whatever the person does in the app meanwhile.
//! Runs are kept in memory (the latest few) for the screen, and when one is over a note about it is kept in its thread
//! (see `records`).
//!
//! Everything that talks to the outside world is behind a trait: [`Runner`] starts the process, [`Host`] tells the
//! window what is happening and brings the vault up to date. The tests give both fakes.

use std::ffi::OsString;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::cli::{self, Search};
use super::config::{self, ClaudeSettings};
use super::prompt::RunRequest;
use super::records::{self, RunFacts, RunRecord};
use super::redact::Redactor;
use super::render::{self, ResultInfo, Step};
use super::runner::{self, Exit, Process, Runner, SpawnSpec};
use super::versions::{Baseline, Change};
use crate::error::{Error, Result};
use crate::{deliverables, ids, settings};

/// How many runs may be going at once.
pub const MAX_CONCURRENT: usize = 3;
/// How many finished runs the screen can still show.
const KEPT_RUNS: usize = 30;
/// How much output is kept in memory for the screen.
pub const MAX_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_LINE_BYTES: usize = 4 * 1024 * 1024;
/// How many steps (tool calls) are remembered for the note about the run; it lists fewer than this.
const MAX_STEPS_KEPT: usize = 2000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- what the screen sees ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Running,
    Finished,
    Failed,
    Cancelled,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Finished => "finished",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunView {
    pub id: String,
    pub project: String,
    pub thread: String,
    pub prompt_id: String,
    pub prompt_title: String,
    pub status: Status,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    /// Why it failed or stopped, in a sentence.
    pub message: Option<String>,
    /// Cancel was pressed and the process is being stopped.
    pub cancelling: bool,
    /// How many pieces of output there have been: the screen compares it with the pieces it has seen.
    pub chunks: u64,
    /// Deliverables the app linked to the prompt because the run created them.
    pub linked: Vec<String>,
    /// Files changed on disk and the vault was reloaded.
    pub vault_reloaded: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunSnapshot {
    pub run: RunView,
    pub output: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    pub seq: u64,
    pub text: String,
}

/// The `claude-run` event: the run as it now is, and the piece of output that was just added, if any.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEvent {
    pub run: RunView,
    pub chunk: Option<Chunk>,
}

/// What putting the vault in order after a run found.
#[derive(Debug, Clone, Default)]
pub struct VaultOutcome {
    pub reloaded: bool,
    /// The versions the run's deliverables were published as and linked to the prompt, like "plan@v0.2.md".
    pub linked: Vec<String>,
    /// Versions made to keep edits that no version held, before the run changed the file. They are not linked.
    pub kept: Vec<String>,
    pub error: Option<String>,
}

/// The world outside the runs: the window, and the vault.
pub trait Host: Send + Sync {
    fn emit(&self, event: &RunEvent);
    /// The process is gone: read in what it wrote, keep any earlier edits it changed as a version, and (when `link`)
    /// publish what the run made of each deliverable and link that version to the prompt.
    fn vault_after_run(&self, request: &RunRequest, changes: &[Change], link: bool) -> VaultOutcome;
    /// Keeps the note about the run in its thread. The error is a sentence for the run's output.
    fn keep_record(&self, _record: &RunRecord) -> std::result::Result<(), String> {
        Ok(())
    }
}

// ---- one run ----------------------------------------------------------------------------------------

struct State {
    status: Status,
    output: String,
    chunks: u64,
    truncated: bool,
    finished_at: Option<String>,
    exit_code: Option<i32>,
    message: Option<String>,
    linked: Vec<String>,
    vault_reloaded: bool,
    result: Option<ResultInfo>,
    /// What the run did, in order, and the model it started with: the note about the run is made from these.
    steps: Vec<Step>,
    model: Option<String>,
}

struct Run {
    id: String,
    project: String,
    thread: String,
    prompt_id: String,
    prompt_title: String,
    started_at: String,
    started: Instant,
    cancel: AtomicBool,
    /// The program printed "unknown option": the Claude Code installed is probably older than the flags used.
    unknown_option: AtomicBool,
    process: Mutex<Option<Arc<dyn Process>>>,
    state: Mutex<State>,
}

impl Run {
    fn new(request: &RunRequest) -> Run {
        Run {
            id: ids::new_id(),
            project: request.project.clone(),
            thread: request.thread.clone(),
            prompt_id: request.prompt_id.clone(),
            prompt_title: request.prompt_title.clone(),
            started_at: ids::now(),
            started: Instant::now(),
            cancel: AtomicBool::new(false),
            unknown_option: AtomicBool::new(false),
            process: Mutex::new(None),
            state: Mutex::new(State {
                status: Status::Running,
                output: String::new(),
                chunks: 0,
                truncated: false,
                finished_at: None,
                exit_code: None,
                message: None,
                linked: Vec::new(),
                vault_reloaded: false,
                result: None,
                steps: Vec::new(),
                model: None,
            }),
        }
    }

    fn view_of(&self, state: &State) -> RunView {
        RunView {
            id: self.id.clone(),
            project: self.project.clone(),
            thread: self.thread.clone(),
            prompt_id: self.prompt_id.clone(),
            prompt_title: self.prompt_title.clone(),
            status: state.status,
            started_at: self.started_at.clone(),
            finished_at: state.finished_at.clone(),
            exit_code: state.exit_code,
            message: state.message.clone(),
            cancelling: state.status == Status::Running && self.cancel.load(Ordering::SeqCst),
            chunks: state.chunks,
            linked: state.linked.clone(),
            vault_reloaded: state.vault_reloaded,
        }
    }

    fn view(&self) -> RunView {
        self.view_of(&lock(&self.state))
    }

    fn snapshot(&self) -> RunSnapshot {
        let state = lock(&self.state);
        RunSnapshot { run: self.view_of(&state), output: state.output.clone() }
    }

    fn is_running(&self) -> bool {
        lock(&self.state).status == Status::Running
    }

    fn is_for(&self, request: &RunRequest) -> bool {
        self.project == request.project && self.thread == request.thread && self.prompt_id == request.prompt_id
    }
}

/// Where a run's output goes: the screen (through the host), with secrets taken out on the way.
struct Sink {
    run: Arc<Run>,
    host: Arc<dyn Host>,
    redactor: Redactor,
}

impl Sink {
    /// Adds text to the output and the screen. Everything is done under the run's lock, so pieces reach the screen in
    /// the order they were added whichever stream they came from.
    fn push(&self, text: &str) {
        let text = self.redactor.apply(text);
        if text.is_empty() {
            return;
        }
        let mut state = lock(&self.run.state);
        let piece = if state.truncated {
            String::new()
        } else if state.output.len() + text.len() > MAX_OUTPUT_BYTES {
            state.truncated = true;
            "… The output is cut here.\n".to_string()
        } else {
            text
        };
        if piece.is_empty() {
            return;
        }
        state.output.push_str(&piece);
        let seq = state.chunks;
        state.chunks += 1;
        self.host.emit(&RunEvent { run: self.run.view_of(&state), chunk: Some(Chunk { seq, text: piece }) });
    }
}

// ---- the manager ------------------------------------------------------------------------------------

/// What a run needs besides the prompt: where the vault is and the settings it runs under.
#[derive(Debug, Clone)]
pub struct RunContext {
    pub root: PathBuf,
    pub settings: ClaudeSettings,
}

struct Inner {
    runner: Arc<dyn Runner>,
    host: Arc<dyn Host>,
    runs: Mutex<Vec<Arc<Run>>>,
    /// Ledger is about to restart to install an update: no run may start. Read and set under the `runs` lock, so a
    /// run either is in the list when the hold is taken or doesn't start.
    held: AtomicBool,
}

#[derive(Clone)]
pub struct Runs {
    inner: Arc<Inner>,
}

impl Runs {
    pub fn new(runner: Arc<dyn Runner>, host: Arc<dyn Host>) -> Runs {
        Runs { inner: Arc::new(Inner { runner, host, runs: Mutex::new(Vec::new()), held: AtomicBool::new(false) }) }
    }

    /// The runs still going.
    pub fn running(&self) -> Vec<RunView> {
        lock(&self.inner.runs).iter().filter(|r| r.is_running()).map(|r| r.view()).collect()
    }

    /// For a restart: succeeds when no run is going, and from then on none can start until [`Runs::release_hold`].
    /// Otherwise the runs still going.
    pub fn hold_for_restart(&self) -> std::result::Result<(), Vec<RunView>> {
        let runs = lock(&self.inner.runs);
        let going: Vec<RunView> = runs.iter().filter(|r| r.is_running()).map(|r| r.view()).collect();
        if !going.is_empty() {
            return Err(going);
        }
        self.inner.held.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// The restart didn't happen after all: runs can start again.
    pub fn release_hold(&self) {
        let _runs = lock(&self.inner.runs);
        self.inner.held.store(false, Ordering::SeqCst);
    }

    /// Starts a run in the background. Fails, without starting anything, if Claude Code can't be found, if this prompt
    /// is already running, or if the limit on runs at once is reached.
    pub fn start(&self, mut request: RunRequest, ctx: RunContext, search: &Search) -> Result<RunView> {
        // Checked again under the lock below; this saves writing the guard file for nothing.
        if self.inner.held.load(Ordering::SeqCst) {
            return Err(restarting());
        }
        let program = cli::resolve(&ctx.settings.cli_path, search)?;
        let _ = settings::ensure_gitignore(&ctx.root);
        let guard = if ctx.settings.confine { Some(write_guard(&ctx)?) } else { None };
        let spec = SpawnSpec {
            env: child_env(&program),
            program,
            args: cli::run_args(&ctx.settings, guard.as_deref()),
            cwd: request.cwd.clone(),
            stdin: request.text.clone().into_bytes(),
        };

        // How the deliverables stand before the run, to tell afterwards what it changed and what to publish.
        let baseline = Baseline::capture(&deliverables::deliverables_dir(&ctx.root, &request.project));

        let run = Arc::new(Run::new(&request));
        request.run_id = run.id.clone();
        {
            let mut runs = lock(&self.inner.runs);
            if self.inner.held.load(Ordering::SeqCst) {
                return Err(restarting());
            }
            if runs.iter().any(|r| r.is_running() && r.is_for(&request)) {
                return Err(Error::invalid("This prompt is already running in Claude Code"));
            }
            let going = runs.iter().filter(|r| r.is_running()).count();
            if going >= MAX_CONCURRENT {
                return Err(Error::invalid(format!(
                    "{going} runs are already going, which is the most at once. Wait for one to finish, or cancel one."
                )));
            }
            runs.push(run.clone());
            // Only the latest few finished runs are kept for the screen.
            let finished = runs.iter().filter(|r| !r.is_running()).count();
            if finished > KEPT_RUNS {
                let mut excess = finished - KEPT_RUNS;
                runs.retain(|r| {
                    if excess > 0 && !r.is_running() {
                        excess -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
        }

        let view = run.view();
        self.inner.host.emit(&RunEvent { run: view.clone(), chunk: None });
        let worker = {
            let (inner, run) = (self.inner.clone(), run.clone());
            thread::Builder::new().name("claude-run".into()).spawn(move || execute(inner, run, spec, request, ctx, baseline))
        };
        if let Err(e) = worker {
            {
                let mut state = lock(&run.state);
                state.status = Status::Failed;
                state.finished_at = Some(ids::now());
                state.message = Some(format!("Couldn't start a thread for the run: {e}"));
            }
            self.inner.host.emit(&RunEvent { run: run.view(), chunk: None });
        }
        Ok(run.view())
    }

    fn find(&self, id: &str) -> Result<Arc<Run>> {
        lock(&self.inner.runs).iter().find(|r| r.id == id).cloned().ok_or_else(|| Error::not_found("That run is no longer listed"))
    }

    /// Every run still listed, oldest first, with its output so far.
    pub fn list(&self) -> Vec<RunSnapshot> {
        let runs: Vec<Arc<Run>> = lock(&self.inner.runs).clone();
        runs.iter().map(|r| r.snapshot()).collect()
    }

    pub fn snapshot(&self, id: &str) -> Result<RunSnapshot> {
        Ok(self.find(id)?.snapshot())
    }

    /// Stops the run: the process and everything it started. The run ends as cancelled once the process is gone.
    pub fn cancel(&self, id: &str) -> Result<RunView> {
        let run = self.find(id)?;
        if run.is_running() && !run.cancel.swap(true, Ordering::SeqCst) {
            self.inner.host.emit(&RunEvent { run: run.view(), chunk: None });
            // Stopping a process tree can take a moment: not on the caller's thread.
            let process = lock(&run.process).clone();
            if let Some(process) = process {
                thread::spawn(move || process.kill_tree());
            }
            // Without a process yet, the worker notices the flag as soon as it has started one.
        }
        Ok(run.view())
    }

    /// Stops every run, and returns once the processes have been told. For when the app closes.
    pub fn cancel_all(&self) {
        let runs: Vec<Arc<Run>> = lock(&self.inner.runs).clone();
        for run in runs.iter().filter(|r| r.is_running()) {
            run.cancel.store(true, Ordering::SeqCst);
            let process = lock(&run.process).clone();
            if let Some(process) = process {
                process.kill_tree();
            }
        }
    }
}

fn restarting() -> Error {
    Error::invalid("Ledger is restarting to install an update. Run the prompt again once it is back.")
}

/// The environment the program is started with: the app's own, plus no colour codes in the output, and the folder the
/// program lives in at the front of the PATH. A GUI app on macOS has a short PATH, and an npm install of Claude Code
/// is a script that needs `node`, which usually sits in the same folder.
pub fn child_env(program: &Path) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = vec![("NO_COLOR".into(), "1".into())];
    if let Some(dir) = program.parent().filter(|d| !d.as_os_str().is_empty()) {
        let existing = std::env::var_os("PATH").unwrap_or_default();
        let paths = std::iter::once(dir.to_path_buf()).chain(std::env::split_paths(&existing));
        if let Ok(joined) = std::env::join_paths(paths) {
            env.push(("PATH".into(), joined));
        }
    }
    env
}

/// The guard rails' settings file, rewritten for each run (it is small, and shown in the confirmation).
fn write_guard(ctx: &RunContext) -> Result<PathBuf> {
    let file = config::guard_path(&ctx.root);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(&file, cli::guard_settings(&ctx.root)).map_err(|e| Error::io(&file, e))?;
    Ok(file)
}

// ---- the worker -------------------------------------------------------------------------------------

fn execute(inner: Arc<Inner>, run: Arc<Run>, spec: SpawnSpec, request: RunRequest, ctx: RunContext, baseline: Baseline) {
    let sink = Arc::new(Sink {
        run: run.clone(),
        host: inner.host.clone(),
        redactor: Redactor::for_vault(&ctx.root),
    });

    let spawned = match inner.runner.spawn(&spec) {
        Ok(spawned) => spawned,
        Err(e) => {
            let reason = spawn_failure(&e, &spec);
            sink.push(&format!("✗ {reason}\n"));
            finish(&inner, &sink, &request, &ctx, &baseline, Status::Failed, Some(reason), None);
            return;
        }
    };
    *lock(&run.process) = Some(spawned.process.clone());
    if run.cancel.load(Ordering::SeqCst) {
        spawned.process.kill_tree();
    }

    let (done_tx, done_rx) = mpsc::channel::<()>();
    for (stream, is_stdout) in [(spawned.stdout, true), (spawned.stderr, false)] {
        let (sink, done) = (sink.clone(), done_tx.clone());
        let started = thread::Builder::new().name("claude-output".into()).spawn(move || {
            read_stream(stream, is_stdout, &sink);
            let _ = done.send(());
        });
        if started.is_err() {
            let _ = done_tx.send(());
        }
    }
    drop(done_tx);

    let exit = spawned.process.wait();
    // The readers end when the pipes close. A program started by the run could hold them open after Claude Code is
    // gone, so they get a moment to finish, not forever.
    let deadline = Instant::now() + Duration::from_secs(2);
    for _ in 0..2 {
        let _ = done_rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    }

    let cancelled = run.cancel.load(Ordering::SeqCst);
    let result = lock(&run.state).result.clone();
    let (status, message, code) = outcome(cancelled, &exit, result.as_ref(), ctx.settings.confine && run.unknown_option.load(Ordering::SeqCst));
    sink.push(&render::summary(result.as_ref(), run.started.elapsed().as_secs()));
    finish(&inner, &sink, &request, &ctx, &baseline, status, message, code);
}

/// Reads one stream to its end, a line at a time, into the run's output.
fn read_stream(stream: Box<dyn Read + Send>, is_stdout: bool, sink: &Sink) {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::new();
    while let Ok(true) = runner::read_line(&mut reader, &mut line, MAX_LINE_BYTES) {
        let text = String::from_utf8_lossy(&line).into_owned();
        if is_stdout {
            let rendered = render::render_line(&text);
            {
                let mut state = lock(&sink.run.state);
                if let Some(result) = rendered.result {
                    state.result = Some(result);
                }
                if state.model.is_none() {
                    state.model = rendered.model;
                }
                let room = MAX_STEPS_KEPT.saturating_sub(state.steps.len());
                state.steps.extend(rendered.steps.into_iter().take(room));
            }
            sink.push(&rendered.text);
        } else {
            let text = text.trim_end();
            if text.to_ascii_lowercase().contains("unknown option") {
                sink.run.unknown_option.store(true, Ordering::SeqCst);
            }
            if !text.is_empty() {
                sink.push(&format!("stderr: {text}\n"));
            }
        }
    }
}

/// How a run ended, from what the process did and what Claude Code said last.
pub fn outcome(cancelled: bool, exit: &io::Result<Exit>, result: Option<&ResultInfo>, old_cli_hint: bool) -> (Status, Option<String>, Option<i32>) {
    let code = exit.as_ref().ok().and_then(|e| e.code);
    if cancelled {
        return (Status::Cancelled, Some("Stopped at your request".to_string()), code);
    }
    let reason = result.filter(|r| r.is_error).map(|r| r.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string());
    match exit {
        Err(e) => (Status::Failed, Some(format!("Couldn't wait for Claude Code: {e}")), None),
        Ok(exit) if exit.success() => match reason {
            None => (Status::Finished, None, code),
            Some(reason) if reason.is_empty() => (Status::Failed, Some("Claude Code reported an error".to_string()), code),
            Some(reason) => (Status::Failed, Some(reason), code),
        },
        Ok(exit) => {
            let mut message = format!("Claude Code stopped with {}", exit.describe());
            if let Some(reason) = reason.filter(|r| !r.is_empty()) {
                message.push_str(&format!(": {reason}"));
            }
            if old_cli_hint {
                message.push_str(". The Claude Code installed may be too old for the extra guard rails: update it, or turn them off under Settings, Claude Code.");
            }
            (Status::Failed, Some(message), code)
        }
    }
}

fn spawn_failure(e: &io::Error, spec: &SpawnSpec) -> String {
    let program = spec.program.display();
    match e.kind() {
        io::ErrorKind::NotFound => format!("Claude Code couldn't be started: {program} is gone. Check the path under Settings, Claude Code."),
        io::ErrorKind::PermissionDenied => format!("Claude Code couldn't be started: no permission to run {program}."),
        _ => format!("Claude Code couldn't be started ({program}): {e}"),
    }
}

/// The last step: bring the vault up to date with what the run wrote (publishing and linking the deliverables), and
/// only then say the run is over, so that the screen, refreshing on that news, sees the new files.
#[allow(clippy::too_many_arguments)]
fn finish(inner: &Inner, sink: &Sink, request: &RunRequest, ctx: &RunContext, baseline: &Baseline, status: Status, message: Option<String>, code: Option<i32>) {
    let run = &sink.run;
    let changes: Vec<Change> = baseline.changes(&deliverables::deliverables_dir(&ctx.root, &request.project));
    let vault = inner.host.vault_after_run(request, &changes, status == Status::Finished);
    if let Some(error) = &vault.error {
        sink.push(&format!("✗ After the run: {error}\n"));
    }
    for kept in &vault.kept {
        sink.push(&format!("Edits of yours that no version held were published as {kept} before the run changed the file\n"));
    }
    if !vault.linked.is_empty() {
        sink.push(&format!("Published, and linked to the prompt: {}\n", vault.linked.join(", ")));
    } else if !changes.is_empty() {
        let paths: Vec<&str> = changes.iter().map(Change::path).collect();
        sink.push(&format!("Changed in deliverables/: {}\n", paths.join(", ")));
    }
    // What the note lists: the versions now linked to the prompt, or else the files the run changed.
    let files: Vec<String> = if vault.linked.is_empty() { changes.iter().map(|c| c.path().to_string()).collect() } else { vault.linked.clone() };

    // The note about the run, in its thread: made from what the run did, and kept before the run is called over.
    let finished_at = ids::now();
    let record = {
        let state = lock(&run.state);
        let model = state.model.clone().unwrap_or_else(|| ctx.settings.model.clone());
        records::build(
            request,
            &RunFacts {
                id: &run.id,
                status: status.as_str(),
                message: message.as_deref(),
                model: &model,
                permission_mode: ctx.settings.permission_mode.as_str(),
                started_at: &run.started_at,
                finished_at: &finished_at,
                seconds: run.started.elapsed().as_secs(),
                result: state.result.as_ref(),
                steps: &state.steps,
                files: &files,
            },
            &sink.redactor,
        )
    };
    if let Err(e) = inner.host.keep_record(&record) {
        sink.push(&format!("✗ The note about this run couldn't be kept in the thread: {e}\n"));
    }

    {
        let mut state = lock(&run.state);
        state.status = status;
        state.finished_at = Some(finished_at);
        state.exit_code = code;
        state.message = message;
        state.linked = vault.linked.clone();
        state.vault_reloaded = vault.reloaded;
        inner.host.emit(&RunEvent { run: run.view_of(&state), chunk: None });
    }
    *lock(&run.process) = None;
}
