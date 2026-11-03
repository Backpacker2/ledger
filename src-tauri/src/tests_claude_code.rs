//! Running a prompt with Claude Code: the settings, the command line, finding the program, what is sent and where,
//! the runs with a fake program (streaming, failing, cancelling, limits), the notes kept about them in the thread,
//! secrets, and a few runs of real processes. Nothing here starts the real `claude`: the real-process tests start this test binary itself.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::{TempDir, tempdir};

use crate::claude_code::cli::{self, Search};
use crate::claude_code::config::{self, ClaudeSettings, PermissionMode};
use crate::claude_code::prompt::{self, RunRequest};
use crate::claude_code::records::{self, RunFacts, RunRecord};
use crate::claude_code::redact::Redactor;
use crate::claude_code::render::{self, ResultInfo, Step};
use crate::claude_code::runner::{self, Exit, OsRunner, Process, Runner, SpawnSpec, Spawned};
use crate::claude_code::runs::{self, Host, MAX_CONCURRENT, MAX_OUTPUT_BYTES, RunContext, RunEvent, RunSnapshot, RunView, Runs, Status, VaultOutcome};
use crate::claude_code::versions::{Baseline, Change};
use crate::claude_code::{check_with, finish};
use crate::error::Result;
use crate::models::Bump;
use crate::state::Core;
use crate::{deliverables, ids, projects, prompts, provenance, templates, threads};

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Text that would break a careless command line: quotes, `&`, `%PATH%`, a newline, a backtick, non-ASCII.
const HOSTILE: &str = "He said \"hi\" & echo %PATH% && del *.* ; $(whoami) `id` 'single'\nsecond line\twith a tab and ^caret | pipe > redirect\nnon-ASCII: héllo wörld, 日本語, 🙂";

// ---- a vault with a prompt in it ----------------------------------------------------------------------

struct Fixture {
    _dir: TempDir,
    _config: TempDir,
    core: Core,
    project: String,
    thread: String,
    prompt: crate::models::PromptView,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture::with_body(HOSTILE)
    }

    fn with_body(body: &str) -> Fixture {
        let dir = tempdir().unwrap();
        let config = tempdir().unwrap();
        let core = Core::open_at(dir.path().join("vault"), config.path().to_path_buf(), false).unwrap();
        let project = projects::create(&core.conn, "Acme").unwrap().slug;
        let thread = threads::create(&core.conn, &project, "Call notes").unwrap().slug;
        let prompt = prompts::create(&core.conn, &project, &thread, "Call summary", body, None).unwrap();
        let mut f = Fixture { _dir: dir, _config: config, core, project, thread, prompt };
        f.persist();
        f
    }

    fn persist(&mut self) {
        self.core.vault.persist(&self.core.conn).unwrap();
    }

    fn root(&self) -> PathBuf {
        self.core.vault.root().to_path_buf()
    }

    fn project_dir(&self) -> PathBuf {
        self.root().join("projects").join(&self.project)
    }

    fn request_for(&self, prompt_id: &str) -> Result<RunRequest> {
        prompt::prepare(&self.core.conn, self.core.vault.root(), &self.project, &self.thread, prompt_id)
    }

    fn request(&self) -> RunRequest {
        self.request_for(&self.prompt.id).unwrap()
    }

    fn more_prompts(&self, n: usize) -> Vec<String> {
        (0..n).map(|i| prompts::create(&self.core.conn, &self.project, &self.thread, &format!("Extra {i}"), "Another request.", None).unwrap().id).collect()
    }

    fn context(&self, settings: ClaudeSettings) -> RunContext {
        RunContext { root: self.root(), settings }
    }
}

// ---- a fake program ---------------------------------------------------------------------------------------

/// A stream the test feeds line by line; it ends when the process "ends".
struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    at: usize,
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.at >= self.pending.len() {
            match self.rx.recv() {
                Ok(bytes) => {
                    self.pending = bytes;
                    self.at = 0;
                }
                Err(_) => return Ok(0),
            }
        }
        let n = out.len().min(self.pending.len() - self.at);
        out[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// The test's hold on a fake process.
struct Handle {
    out: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    err: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    exit: Mutex<Option<Exit>>,
    ended: Condvar,
    killed: AtomicBool,
}

impl Handle {
    fn stdout(&self, line: &str) {
        if let Some(tx) = &*lock(&self.out) {
            let _ = tx.send(format!("{line}\n").into_bytes());
        }
    }

    fn stderr(&self, line: &str) {
        if let Some(tx) = &*lock(&self.err) {
            let _ = tx.send(format!("{line}\n").into_bytes());
        }
    }

    fn end(&self, exit: Exit) {
        lock(&self.out).take();
        lock(&self.err).take();
        lock(&self.exit).get_or_insert(exit);
        self.ended.notify_all();
    }

    fn exit(&self, code: i32) {
        self.end(Exit { code: Some(code), signal: None });
    }

    fn killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }
}

struct FakeProcess(Arc<Handle>);

impl Process for FakeProcess {
    fn wait(&self) -> io::Result<Exit> {
        let mut exit = lock(&self.0.exit);
        loop {
            if let Some(done) = *exit {
                return Ok(done);
            }
            exit = self.0.ended.wait(exit).unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn kill_tree(&self) {
        self.0.killed.store(true, Ordering::SeqCst);
        self.0.end(Exit { code: None, signal: Some(15) });
    }
}

#[derive(Default)]
struct FakeRunner {
    specs: Mutex<Vec<SpawnSpec>>,
    handles: Mutex<Vec<Arc<Handle>>>,
    fail: Mutex<Option<io::ErrorKind>>,
    /// Stdout lines, stderr lines and the exit code to play back at once, instead of waiting for the test.
    script: Mutex<Option<(Vec<String>, Vec<String>, i32)>>,
}

impl Runner for FakeRunner {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<Spawned> {
        lock(&self.specs).push(spec.clone());
        if let Some(kind) = *lock(&self.fail) {
            return Err(io::Error::new(kind, "fake failure"));
        }
        let (out_tx, out_rx) = mpsc::channel();
        let (err_tx, err_rx) = mpsc::channel();
        let handle = Arc::new(Handle {
            out: Mutex::new(Some(out_tx)),
            err: Mutex::new(Some(err_tx)),
            exit: Mutex::new(None),
            ended: Condvar::new(),
            killed: AtomicBool::new(false),
        });
        if let Some((out, err, code)) = lock(&self.script).clone() {
            out.iter().for_each(|l| handle.stdout(l));
            err.iter().for_each(|l| handle.stderr(l));
            handle.exit(code);
        }
        lock(&self.handles).push(handle.clone());
        Ok(Spawned {
            stdout: Box::new(ChannelReader { rx: out_rx, pending: Vec::new(), at: 0 }),
            stderr: Box::new(ChannelReader { rx: err_rx, pending: Vec::new(), at: 0 }),
            process: Arc::new(FakeProcess(handle)),
        })
    }
}

impl FakeRunner {
    fn scripted(out: &[&str], err: &[&str], code: i32) -> FakeRunner {
        let runner = FakeRunner::default();
        *lock(&runner.script) = Some((out.iter().map(|s| s.to_string()).collect(), err.iter().map(|s| s.to_string()).collect(), code));
        runner
    }

    /// The nth process started, once it has been.
    fn handle(&self, n: usize) -> Arc<Handle> {
        wait_until("the process to start", || lock(&self.handles).len() > n);
        lock(&self.handles)[n].clone()
    }

    fn spec(&self, n: usize) -> SpawnSpec {
        wait_until("the process to start", || lock(&self.specs).len() > n);
        lock(&self.specs)[n].clone()
    }

    fn spawned(&self) -> usize {
        lock(&self.specs).len()
    }
}

#[derive(Default)]
struct FakeHost {
    events: Mutex<Vec<RunEvent>>,
    vault_calls: Mutex<Vec<(Vec<String>, bool)>>,
    outcome: Mutex<VaultOutcome>,
    /// The notes kept about runs, and a reason to refuse to keep them.
    records: Mutex<Vec<RunRecord>>,
    record_error: Mutex<Option<String>>,
}

impl Host for FakeHost {
    fn emit(&self, event: &RunEvent) {
        lock(&self.events).push(event.clone());
    }

    fn vault_after_run(&self, _request: &RunRequest, changes: &[Change], link: bool) -> VaultOutcome {
        lock(&self.vault_calls).push((changes.iter().map(|c| c.path().to_string()).collect(), link));
        lock(&self.outcome).clone()
    }

    fn keep_record(&self, record: &RunRecord) -> std::result::Result<(), String> {
        if let Some(reason) = lock(&self.record_error).clone() {
            return Err(reason);
        }
        lock(&self.records).push(record.clone());
        Ok(())
    }
}

struct Rig {
    runner: Arc<FakeRunner>,
    host: Arc<FakeHost>,
    runs: Runs,
    search: Search,
    _cli_dir: TempDir,
}

fn make_executable(file: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = file;
}

/// A folder holding a file that is named like Claude Code (its content doesn't matter: it is never started).
fn fake_cli() -> (TempDir, Search) {
    let dir = tempdir().unwrap();
    let file = dir.path().join(cli::candidate_names(cfg!(windows))[0]);
    fs::write(&file, b"not really claude").unwrap();
    make_executable(&file);
    let search = Search { path_var: Some(std::env::join_paths([dir.path()]).unwrap()), fallback_dirs: vec![], windows: cfg!(windows) };
    (dir, search)
}

impl Rig {
    fn new() -> Rig {
        Rig::with(FakeRunner::default())
    }

    fn with(runner: FakeRunner) -> Rig {
        let (cli_dir, search) = fake_cli();
        let runner = Arc::new(runner);
        let host = Arc::new(FakeHost::default());
        let runs = Runs::new(runner.clone(), host.clone());
        Rig { runner, host, runs, search, _cli_dir: cli_dir }
    }

    fn start(&self, f: &Fixture) -> Result<RunView> {
        self.start_with(f, &f.prompt.id, ClaudeSettings::default())
    }

    fn start_with(&self, f: &Fixture, prompt_id: &str, settings: ClaudeSettings) -> Result<RunView> {
        self.runs.start(f.request_for(prompt_id)?, f.context(settings), &self.search)
    }

    fn snapshot(&self, id: &str) -> RunSnapshot {
        self.runs.snapshot(id).unwrap()
    }

    fn wait_for(&self, id: &str, status: Status) -> RunSnapshot {
        wait_until(&format!("the run to be {status:?}"), || self.runs.snapshot(id).unwrap().run.status == status);
        self.snapshot(id)
    }

    fn events(&self) -> Vec<RunEvent> {
        lock(&self.host.events).clone()
    }
}

const INIT: &str = r#"{"type":"system","subtype":"init","model":"claude-test","permissionMode":"acceptEdits","tools":["Read","Write"]}"#;
const SAID: &str = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"I'll write the summary."},{"type":"tool_use","name":"Write","input":{"file_path":"deliverables/summary.md","content":"Summary of the call"}}]}}"#;
const RESULT_OK: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"Wrote deliverables/summary.md","num_turns":2,"total_cost_usd":0.0421,"permission_denials":[{"tool_name":"Bash","tool_input":{"command":"curl example.com"}}]}"#;
const RESULT_ERROR: &str = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Invalid API key. Please run /login","num_turns":0}"#;

// ---- settings -----------------------------------------------------------------------------------------

#[test]
fn without_a_settings_file_the_defaults_apply() {
    let dir = tempdir().unwrap();
    let settings = config::load(dir.path());
    assert_eq!(settings, ClaudeSettings::default());
    assert_eq!(settings.permission_mode, PermissionMode::AcceptEdits);
    assert!(settings.confine);
    assert!(!settings.auto_run, "a prompt is only run by itself when it was asked for");
    assert!(settings.cli_path.is_empty() && settings.model.is_empty());
}

#[test]
fn settings_persist_in_the_ledger_folder_which_git_ignores() {
    let dir = tempdir().unwrap();
    let saved = ClaudeSettings::validated(" \"C:\\Program Files\\Claude\\claude.exe\" ", "plan", " claude-sonnet-4-5 ", false, true).unwrap();
    assert_eq!(saved.cli_path, "C:\\Program Files\\Claude\\claude.exe", "pasted quotes are dropped");
    config::save(dir.path(), &saved).unwrap();

    assert_eq!(config::settings_path(dir.path()), dir.path().join(".ledger").join("claude-code.json"));
    assert!(config::settings_path(dir.path()).is_file());
    assert_eq!(config::load(dir.path()), saved);
    assert_eq!(config::load(dir.path()).permission_mode, PermissionMode::Plan);
    assert!(config::load(dir.path()).auto_run, "running a prompt when it is created is remembered");
    assert!(fs::read_to_string(config::settings_path(dir.path())).unwrap().contains("\"autoRun\": true"));
    config::save(dir.path(), &ClaudeSettings { auto_run: false, ..saved }).unwrap();
    assert!(!config::load(dir.path()).auto_run);

    // `.ledger/` is what the vault's .gitignore lists, so the file is neither committed nor synced.
    let ignore = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l.trim() == ".ledger/"), "{ignore}");
}

#[test]
fn an_old_or_damaged_settings_file_loads_field_by_field() {
    let dir = tempdir().unwrap();
    let write = |text: &str| {
        fs::create_dir_all(dir.path().join(".ledger")).unwrap();
        fs::write(config::settings_path(dir.path()), text).unwrap();
    };

    // An older file with only some of the settings: the rest are the defaults.
    write(r#"{"cliPath": "/opt/claude/bin/claude"}"#);
    let old = config::load(dir.path());
    assert_eq!(old.cli_path, "/opt/claude/bin/claude");
    assert_eq!((old.permission_mode, old.model.as_str(), old.confine, old.auto_run), (PermissionMode::AcceptEdits, "", true, false));

    // Not JSON, empty, or not an object at all.
    for broken in ["", "{", "not json", "[1, 2]", "42", "null"] {
        write(broken);
        assert_eq!(config::load(dir.path()), ClaudeSettings::default(), "{broken:?}");
    }

    // One bad field doesn't spoil the others, and unknown fields are ignored.
    write(r#"{"cliPath": 5, "model": "opus", "permissionMode": ["x"], "confine": "yes", "autoRun": "yes", "somethingNew": {"a": 1}}"#);
    let mixed = config::load(dir.path());
    assert_eq!((mixed.cli_path.as_str(), mixed.model.as_str(), mixed.permission_mode, mixed.confine), ("", "opus", PermissionMode::AcceptEdits, true));
    assert!(!mixed.auto_run, "anything but a real true leaves it off");

    // A hand-edited file can't switch the permission checks off, or put an option in as the model.
    write(r#"{"permissionMode": "bypassPermissions", "model": "--dangerously-skip-permissions"}"#);
    let sneaky = config::load(dir.path());
    assert_eq!(sneaky.permission_mode, PermissionMode::AcceptEdits);
    assert_eq!(sneaky.model, "");
}

#[test]
fn what_is_typed_in_settings_is_checked_because_it_reaches_the_command_line() {
    for fine in ["sonnet", "opus", "claude-sonnet-4-5", "claude-opus-4-1-20250805", "sonnet[1m]", "us.anthropic.claude-sonnet-4-5-v1:0"] {
        assert!(ClaudeSettings::validated("", "acceptEdits", fine, true, false).is_ok(), "{fine}");
    }
    let too_long = "m".repeat(101);
    for bad in ["--dangerously-skip-permissions", "-p", "two words", "a&b", "%PATH%", "\"quoted\"", "x\ny", "a;b", "$(id)", "é", too_long.as_str()] {
        assert!(ClaudeSettings::validated("", "acceptEdits", bad, true, false).is_err(), "{bad:?}");
    }
    for mode in ["bypassPermissions", "default", "auto", "", "AcceptEdits", "--dangerously-skip-permissions"] {
        assert!(ClaudeSettings::validated("", mode, "", true, false).is_err(), "{mode:?}");
    }
    assert!(ClaudeSettings::validated("a\nb", "plan", "", true, false).is_err());
    assert_eq!(ClaudeSettings::validated("", "dontAsk", "", true, false).unwrap().permission_mode, PermissionMode::DontAsk);
}

// ---- the command line ---------------------------------------------------------------------------------

#[test]
fn the_command_line_is_a_fixed_set_of_flags_and_the_chosen_values() {
    let plain = ClaudeSettings { confine: false, ..Default::default() };
    assert_eq!(cli::run_args(&plain, None), os(&["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits"]));

    let all = ClaudeSettings { permission_mode: PermissionMode::Plan, model: "opus".into(), confine: true, cli_path: String::new(), auto_run: true };
    assert_eq!(
        cli::run_args(&all, Some(Path::new("/vault/.ledger/runs/claude-guard.json"))),
        os(&[
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "plan",
            "--model",
            "opus",
            "--setting-sources",
            "user",
            "--settings",
            "/vault/.ledger/runs/claude-guard.json",
            "--strict-mcp-config"
        ])
    );
    assert_eq!(cli::version_args(), os(&["--version"]));
}

#[test]
fn no_combination_of_settings_can_skip_the_permission_checks() {
    for mode in [PermissionMode::AcceptEdits, PermissionMode::Plan, PermissionMode::DontAsk] {
        for model in ["", "sonnet"] {
            for confine in [false, true] {
                let settings = ClaudeSettings { permission_mode: mode, model: model.into(), confine, cli_path: String::new(), auto_run: false };
                let args = cli::run_args(&settings, confine.then(|| Path::new("guard.json")));
                for arg in &args {
                    let arg = arg.to_string_lossy();
                    for forbidden in ["dangerously", "bypassPermissions", "--add-dir", "allowedTools", "allowed-tools"] {
                        assert!(!arg.contains(forbidden), "{arg} in {args:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_prompt_goes_on_standard_input_whatever_it_contains() {
    let f = Fixture::new();
    let rig = Rig::new();
    let run = rig.start(&f).unwrap();
    let spec = rig.runner.spec(0);

    // Exactly the prompt, byte for byte: quotes, `&`, `%PATH%`, newlines, a backtick and non-ASCII included.
    let sent = String::from_utf8(spec.stdin.clone()).unwrap();
    assert!(sent.contains(HOSTILE), "{sent}");
    assert!(sent.ends_with(&format!("{HOSTILE}\n")));
    assert_eq!(sent, f.request().text);

    // And none of it is in the arguments, which are the fixed flags and nothing else.
    for arg in &spec.args {
        let arg = arg.to_string_lossy();
        assert!(!arg.contains("%PATH%") && !arg.contains("whoami") && !arg.contains("héllo"), "{arg}");
    }
    let guard = config::guard_path(&f.root());
    assert_eq!(spec.args, cli::run_args(&ClaudeSettings::default(), Some(guard.as_path())));
    // The program is the one found, started on its own (no shell is named anywhere).
    assert_eq!(spec.program.file_name().unwrap(), cli::candidate_names(cfg!(windows))[0]);
    // Its own folder leads the PATH it is given (an npm install needs the `node` that sits next to it).
    let path = spec.env.iter().find(|(name, _)| name == "PATH").map(|(_, value)| value.clone()).expect("the child gets a PATH");
    assert_eq!(std::env::split_paths(&path).next().unwrap(), spec.program.parent().unwrap());
    assert!(spec.env.iter().any(|(name, value)| name == "NO_COLOR" && value == "1"));
    assert_eq!(run.status, Status::Running);
    rig.runs.cancel_all();
}

#[test]
fn the_command_is_shown_for_reading_with_quotes_where_they_are_needed() {
    let shown = cli::display_command(Path::new("C:\\Program Files\\Claude\\claude.exe"), &os(&["-p", "--settings", "C:\\a b\\g.json", "--model", "sonnet[1m]"]));
    assert_eq!(shown, "\"C:\\Program Files\\Claude\\claude.exe\" -p --settings \"C:\\a b\\g.json\" --model \"sonnet[1m]\"");
}

#[test]
fn guard_rails_name_the_vaults_secrets_and_confine_reads_and_writes() {
    assert_eq!(cli::rule_path(r"C:\Users\Me\Ledger\.ledger").as_deref(), Some("//c/Users/Me/Ledger/.ledger"));
    assert_eq!(cli::rule_path(r"\\?\D:\Vaults\x").as_deref(), Some("//d/Vaults/x"));
    assert_eq!(cli::rule_path("/Users/me/Ledger/.ledger").as_deref(), Some("//Users/me/Ledger/.ledger"));
    for unusable in [r"\\server\share\x", "relative/dir", "/with/a*star", "C:rel"] {
        assert_eq!(cli::rule_path(unusable), None, "{unusable}");
    }

    let root = tempdir().unwrap();
    let guard: serde_json::Value = serde_json::from_str(&cli::guard_settings(root.path())).unwrap();
    let deny: Vec<&str> = guard["permissions"]["deny"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(deny.contains(&"Read(**/.env)") && deny.contains(&"Read(**/.env.*)"), "{deny:?}");
    let ledger = cli::rule_path(&root.path().join(".ledger").display().to_string()).expect("a temp dir has a usable path");
    assert!(deny.contains(&format!("Read({ledger}/**)").as_str()) && deny.contains(&format!("Edit({ledger}/**)").as_str()), "{deny:?}");
    assert_eq!(guard["permissions"]["blockReadsOutsideWorkingDirectories"], true);
    assert_eq!(guard["permissions"]["blockWritesOutsideWorkingDirectories"], true);
}

#[test]
fn a_run_with_guard_rails_writes_the_guard_file_and_points_at_it_without_them_it_doesnt() {
    let f = Fixture::new();
    let rig = Rig::with(FakeRunner::scripted(&[], &[], 0));
    let guarded = rig.start(&f).unwrap();
    rig.wait_for(&guarded.id, Status::Finished);
    let spec = rig.runner.spec(0);
    let at = spec.args.iter().position(|a| a == "--settings").expect("--settings is passed");
    let file = PathBuf::from(&spec.args[at + 1]);
    assert_eq!(file, config::guard_path(&f.root()));
    assert!(file.starts_with(f.root().join(".ledger")), "the guard file sits in the folder that is never synced");
    assert_eq!(fs::read_to_string(&file).unwrap(), cli::guard_settings(&f.root()));
    assert!(spec.args.windows(2).any(|w| w[0] == "--setting-sources" && w[1] == "user"));
    assert!(spec.args.iter().any(|a| a == "--strict-mcp-config"), "a .mcp.json in the vault can't start a program");

    let open = ClaudeSettings { confine: false, ..Default::default() };
    let unguarded = rig.start_with(&f, &f.prompt.id, open).unwrap();
    rig.wait_for(&unguarded.id, Status::Finished);
    let spec = rig.runner.spec(1);
    assert!(!spec.args.iter().any(|a| a == "--settings" || a == "--setting-sources" || a == "--strict-mcp-config"), "{:?}", spec.args);
}

// ---- finding the program ------------------------------------------------------------------------------

#[test]
fn a_missing_program_is_a_clear_and_actionable_message() {
    let empty = tempdir().unwrap();
    let search = Search { path_var: Some(std::env::join_paths([empty.path()]).unwrap()), fallback_dirs: vec![], windows: cfg!(windows) };
    let message = cli::resolve("", &search).unwrap_err().to_string();
    assert!(message.contains("wasn't found") && message.contains("PATH") && message.contains("Settings"), "{message}");

    let nowhere = empty.path().join("no").join("claude");
    let message = cli::resolve(&nowhere.display().to_string(), &search).unwrap_err().to_string();
    assert!(message.contains(&nowhere.display().to_string()) && message.contains("Settings"), "{message}");
    assert!(cli::resolve("claude-code-nope", &search).unwrap_err().to_string().contains("claude-code-nope"));
    assert!(cli::resolve("", &Search::default()).is_err(), "no PATH at all");
}

#[test]
fn a_run_without_the_program_starts_nothing() {
    let f = Fixture::new();
    let rig = Rig::new();
    let empty = tempdir().unwrap();
    let search = Search { path_var: Some(std::env::join_paths([empty.path()]).unwrap()), fallback_dirs: vec![], windows: cfg!(windows) };
    let err = rig.runs.start(f.request(), f.context(ClaudeSettings::default()), &search).unwrap_err().to_string();
    assert!(err.contains("Claude Code wasn't found"), "{err}");
    assert!(rig.runs.list().is_empty());
    assert_eq!(rig.runner.spawned(), 0);
    assert!(!config::guard_path(&f.root()).exists(), "no guard file");
    assert!(lock(&rig.host.records).is_empty(), "nothing ran, so there is nothing to note");
}

#[test]
fn the_program_is_found_on_the_path_in_install_folders_or_by_the_path_in_settings() {
    let (dir, search) = fake_cli();
    let name = cli::candidate_names(cfg!(windows))[0];
    let expected = dir.path().join(name);
    assert_eq!(cli::resolve("", &search).unwrap(), expected);

    // Not on the PATH, but in a folder the installers use.
    let in_fallback = Search { path_var: None, fallback_dirs: vec![dir.path().to_path_buf()], windows: cfg!(windows) };
    assert_eq!(cli::resolve("", &in_fallback).unwrap(), expected);

    // A path, a folder, and a bare name.
    let no_path = Search { windows: cfg!(windows), ..Default::default() };
    assert_eq!(cli::resolve(&expected.display().to_string(), &no_path).unwrap(), expected);
    assert_eq!(cli::resolve(&dir.path().display().to_string(), &no_path).unwrap(), expected);
    assert_eq!(cli::resolve(name, &search).unwrap(), expected);
}

#[test]
fn on_windows_the_native_exe_is_preferred_over_the_npm_cmd_shim() {
    assert_eq!(cli::candidate_names(true), ["claude.exe", "claude.cmd"]);
    assert_eq!(cli::candidate_names(false), ["claude"]);

    let dir = tempdir().unwrap();
    for name in ["claude.cmd", "claude.exe"] {
        let file = dir.path().join(name);
        fs::write(&file, b"x").unwrap();
        make_executable(&file);
    }
    let search = Search { path_var: Some(std::env::join_paths([dir.path()]).unwrap()), fallback_dirs: vec![], windows: true };
    assert_eq!(cli::resolve("", &search).unwrap(), dir.path().join("claude.exe"));
    fs::remove_file(dir.path().join("claude.exe")).unwrap();
    assert_eq!(cli::resolve("", &search).unwrap(), dir.path().join("claude.cmd"), "only the npm shim is installed");
    // "claude" typed in settings on Windows finds either.
    assert_eq!(cli::resolve("claude", &search).unwrap(), dir.path().join("claude.cmd"));
}

#[test]
fn only_absolute_folders_on_the_path_count() {
    let absolute = tempdir().unwrap();
    let joined = std::env::join_paths([PathBuf::from("."), PathBuf::from(""), PathBuf::from("relative/bin"), absolute.path().to_path_buf()]).unwrap();
    let search = Search { path_var: Some(joined), fallback_dirs: vec![], windows: cfg!(windows) };
    assert_eq!(search.path_dirs(), vec![absolute.path().to_path_buf()]);
}

#[test]
fn the_check_runs_the_version_command_and_says_what_happened() {
    let (dir, search) = fake_cli();
    let cwd = tempdir().unwrap();
    let short = Duration::from_millis(300);

    let good = FakeRunner::scripted(&["2.1.100 (Claude Code)"], &[], 0);
    let check = check_with(&good, "", &search, cwd.path(), short);
    assert!(check.ok, "{}", check.message);
    assert_eq!(check.version, "2.1.100 (Claude Code)");
    assert_eq!(check.path.as_deref(), Some(dir.path().join(cli::candidate_names(cfg!(windows))[0]).display().to_string().as_str()));
    assert_eq!(good.spec(0).args, os(&["--version"]));
    assert_eq!(good.spec(0).cwd, cwd.path());
    assert!(good.spec(0).stdin.is_empty());

    let broken = FakeRunner::scripted(&[], &["boom: not logged in"], 1);
    let check = check_with(&broken, "", &search, cwd.path(), short);
    assert!(!check.ok && check.message.contains("exit code 1") && check.message.contains("boom"), "{}", check.message);

    // A program that never answers is stopped.
    let hangs = FakeRunner::default();
    let check = check_with(&hangs, "", &search, cwd.path(), Duration::from_millis(100));
    assert!(!check.ok && check.message.contains("didn't answer"), "{}", check.message);
    assert!(hangs.handle(0).killed());

    let missing = check_with(&good, &dir.path().join("nope").display().to_string(), &search, cwd.path(), short);
    assert!(!missing.ok && missing.path.is_none());
}

// ---- what is sent, and where --------------------------------------------------------------------------

#[test]
fn the_text_is_a_short_preamble_then_the_prompt_as_written() {
    let f = Fixture::new();
    let text = f.request().text;
    for expected in ["\"Acme\"", "README.md", "deliverables/", &format!("threads/{}/", f.thread), "# Call summary", HOSTILE] {
        assert!(text.contains(expected), "{expected}: {text}");
    }
    assert!(text.len() < HOSTILE.len() + 1200, "the preamble stays short: {} bytes", text.len() - HOSTILE.len());
    assert!(text.find("README.md").unwrap() < text.find(HOSTILE).unwrap(), "the instructions come first");
    assert!(!text.contains("already linked") && !text.contains("template"), "nothing about links or templates when there are none");
}

#[test]
fn linked_deliverables_and_templates_are_mentioned_only_when_they_matter() {
    let f = Fixture::with_body("Write up the call.");
    let skeleton = "# Interview\n\n```text\nfenced\n```\n## Who\n";
    let template = templates::create(&f.core.conn, "", "Interview", "after a call", &[], skeleton).unwrap();
    let prompt = prompts::create(&f.core.conn, &f.project, &f.thread, "Interview notes", "Summarise it.", Some(&template.id)).unwrap();

    // A template and nothing written yet: the structure is included, in a fence long enough to hold the one inside it.
    let text = f.request_for(&prompt.id).unwrap().text;
    assert!(text.contains("the template \"Interview\"") && text.contains("## Who"), "{text}");
    assert!(text.contains("````markdown\n# Interview"), "the fence is longer than the backticks inside: {text}");

    // A deliverable linked but not written yet: named, with the structure still offered.
    let made = deliverables::create(&f.core.conn, &f.project, "Interview notes", "").unwrap();
    prompts::link_deliverable(&f.core.conn, &f.project, &f.thread, &prompt.id, &made.path).unwrap();
    let text = f.request_for(&prompt.id).unwrap().text;
    assert!(text.contains("deliverables/interview-notes.md"), "{text}");
    assert!(!text.contains("(not written yet)"), "it exists in the vault, so it is not marked missing");
    assert!(!text.contains("the template"), "the file exists and already has its structure: {text}");
}

#[test]
fn the_run_happens_in_the_projects_folder() {
    let f = Fixture::new();
    let request = f.request();
    assert_eq!(request.cwd, f.root().join("projects").join(&f.project));
    assert!(request.cwd.is_dir());
    assert!(request.cwd.join("README.md").is_file(), "the reading guide is in it");
    assert!(request.cwd.join("deliverables").is_dir(), "where the deliverable goes is there");

    let rig = Rig::new();
    rig.start(&f).unwrap();
    assert_eq!(rig.runner.spec(0).cwd, request.cwd);
    rig.runs.cancel_all();
}

#[test]
fn a_prompt_that_isnt_there_is_refused_and_nothing_starts() {
    let f = Fixture::new();
    let rig = Rig::new();
    let refuse = |project: &str, thread: &str, id: &str| {
        let err = prompt::prepare(&f.core.conn, f.core.vault.root(), project, thread, id).unwrap_err().to_string();
        assert!(!err.is_empty());
    };
    refuse("nope", &f.thread, &f.prompt.id);
    refuse(&f.project, "no-thread", &f.prompt.id);
    refuse(&f.project, &f.thread, "no-prompt");
    // Names that could lead out of the vault are not projects, threads or prompts.
    for hostile in ["..", ".", "", "a/b", "a\\b", "..\\..", "C:", "x\0y"] {
        refuse(hostile, &f.thread, &f.prompt.id);
        refuse(&f.project, hostile, &f.prompt.id);
        refuse(&f.project, &f.thread, hostile);
    }
    // A project in the database whose folder has gone from the disk.
    fs::remove_dir_all(f.project_dir()).unwrap();
    let err = f.request_for(&f.prompt.id).unwrap_err().to_string();
    assert!(err.contains("folder isn't on disk"), "{err}");
    assert!(rig.runs.list().is_empty() && rig.runner.spawned() == 0);
}

#[test]
fn what_is_sent_is_what_was_shown() {
    let f = Fixture::new();
    let request = f.request();
    assert!(prompt::ensure_confirmed(&request, &request.text).is_ok());
    let err = prompt::ensure_confirmed(&request, "an older text").unwrap_err().to_string();
    assert!(err.contains("changed after you looked"), "{err}");
}

#[test]
fn a_prompt_that_is_far_too_long_is_refused() {
    let f = Fixture::with_body(&"word ".repeat(300_000));
    assert!(f.request_for(&f.prompt.id).unwrap_err().to_string().contains("too long"));
}

#[test]
fn the_files_in_the_deliverables_folder_are_listed_without_dotfiles() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("reports")).unwrap();
    fs::write(dir.path().join("old.md"), "x").unwrap();
    fs::write(dir.path().join("reports").join("q3.md"), "x").unwrap();
    fs::write(dir.path().join(".hidden.md"), "x").unwrap();
    assert_eq!(prompt::list_files(dir.path()), BTreeSet::from(["old.md".to_string(), "reports/q3.md".to_string()]));
}

#[test]
fn a_baseline_tells_new_and_changed_files_from_untouched_ones_and_remembers_unpublished_edits() {
    let dir = tempdir().unwrap();
    let put = |name: &str, text: &str| {
        let file = dir.path().join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, text).unwrap();
    };
    put("clean.md", "published text");
    put("clean@v0.1.md", "published text");
    put("same.md", "same text");
    put("same@v0.1.md", "same text");
    put("dirty.md", "edited since");
    put("dirty@v0.1.md", "the version");
    put("draft.md", "notes nobody published");
    put("stub.md", "# Stub\n");
    put("empty.md", "");
    put(".hidden.md", "x");
    let baseline = Baseline::capture(dir.path());

    put("new.md", "x");
    put("reports/q3.md", "x");
    put("clean.md", "the run's text");
    put("same.md", "same text"); // rewritten with what it said
    put("dirty.md", "the run's text");
    put("draft.md", "the run's text");
    put("stub.md", "# Stub\n\nNow written.");
    put("empty.md", "written");
    put("clean@v0.2.md", "a version made meanwhile"); // versions are never changes
    put(".hidden.md", "changed");

    let edited = |path: &str, held: Option<&str>| Change::Edited { path: path.into(), held: held.map(|h| h.as_bytes().to_vec()) };
    assert_eq!(
        baseline.changes(dir.path()),
        [
            edited("clean.md", None),
            edited("dirty.md", Some("edited since")),
            edited("draft.md", Some("notes nobody published")),
            Change::New("empty.md".into()),
            Change::New("new.md".into()),
            Change::New("reports/q3.md".into()),
            Change::New("stub.md".into()),
        ],
        "a file the run rewrote with the same text isn't a change; a placeholder counts as new; earlier edits are held"
    );
    assert_eq!(Baseline::capture(dir.path()).changes(dir.path()), Vec::<Change>::new(), "nothing happened since");
}

// ---- runs: streaming, finishing, failing --------------------------------------------------------------------

#[test]
fn output_arrives_while_the_run_is_going_and_the_run_finishes() {
    let f = Fixture::new();
    let rig = Rig::new();
    let run = rig.start(&f).unwrap();
    assert_eq!((run.status, run.chunks, run.cancelling), (Status::Running, 0, false));
    let process = rig.runner.handle(0);

    process.stdout(INIT);
    wait_until("the first output", || rig.snapshot(&run.id).output.contains("Started: claude-test"));
    let midway = rig.snapshot(&run.id);
    assert_eq!(midway.run.status, Status::Running, "output is visible while it is still running");

    process.stdout(SAID);
    wait_until("the second output", || rig.snapshot(&run.id).output.contains("→ Write deliverables/summary.md"));
    process.stdout(RESULT_OK);
    process.exit(0);
    let done = rig.wait_for(&run.id, Status::Finished);

    for expected in ["I'll write the summary.", "→ Write deliverables/summary.md", "2 turns", "Refused by the permission mode: Bash curl example.com"] {
        assert!(done.output.contains(expected), "{expected}: {}", done.output);
    }
    // The program reports a dollar figure (RESULT_OK has one); the closing line leaves it out.
    assert!(!done.output.contains('$'), "{}", done.output);
    assert_eq!((done.run.exit_code, done.run.message.clone()), (Some(0), None));
    assert!(done.run.finished_at.is_some());

    // What the window is told: numbered pieces in order, that add up to the output, and the end last.
    let events = rig.events();
    let seqs: Vec<u64> = events.iter().filter_map(|e| e.chunk.as_ref().map(|c| c.seq)).collect();
    assert_eq!(seqs, (0..seqs.len() as u64).collect::<Vec<_>>());
    let joined: String = events.iter().filter_map(|e| e.chunk.as_ref().map(|c| c.text.clone())).collect();
    assert_eq!(joined, done.output);
    assert_eq!(done.run.chunks, seqs.len() as u64);
    assert_eq!(events.last().unwrap().run.status, Status::Finished);
    assert_eq!(events.first().unwrap().run.status, Status::Running);
    // The vault was told once, and the finished run may link what it made.
    assert_eq!(*lock(&rig.host.vault_calls), vec![(Vec::<String>::new(), true)]);
}

#[test]
fn what_the_program_prints_to_stderr_is_shown_and_a_failing_exit_fails_the_run() {
    let f = Fixture::new();
    let rig = Rig::with(FakeRunner::scripted(&[], &["Error: something broke"], 2));
    let run = rig.start(&f).unwrap();
    let done = rig.wait_for(&run.id, Status::Failed);
    assert!(done.output.contains("stderr: Error: something broke"), "{}", done.output);
    assert_eq!(done.run.exit_code, Some(2));
    assert!(done.run.message.as_deref().unwrap().contains("exit code 2"), "{:?}", done.run.message);
    assert_eq!(*lock(&rig.host.vault_calls), vec![(Vec::<String>::new(), false)], "a failed run links nothing");
}

#[test]
fn a_run_that_reports_an_error_fails_even_if_the_program_exits_cleanly() {
    let f = Fixture::new();
    let rig = Rig::with(FakeRunner::scripted(&[INIT, RESULT_ERROR], &[], 0));
    let run = rig.start(&f).unwrap();
    let done = rig.wait_for(&run.id, Status::Failed);
    assert_eq!(done.run.message.as_deref(), Some("Invalid API key. Please run /login"));
    assert!(done.output.contains("✗ Invalid API key. Please run /login"), "{}", done.output);
}

#[test]
fn a_program_that_prints_unknown_option_gets_the_hint_about_the_guard_rails() {
    let f = Fixture::new();
    let rig = Rig::with(FakeRunner::scripted(&[], &["error: unknown option '--setting-sources'"], 1));
    let guarded = rig.start(&f).unwrap();
    let done = rig.wait_for(&guarded.id, Status::Failed);
    assert!(done.run.message.as_deref().unwrap().contains("too old for the extra guard rails"), "{:?}", done.run.message);

    let unguarded = rig.start_with(&f, &f.prompt.id, ClaudeSettings { confine: false, ..Default::default() }).unwrap();
    let done = rig.wait_for(&unguarded.id, Status::Failed);
    assert!(!done.run.message.as_deref().unwrap().contains("guard rails"), "no hint when the guard rails were not used");
}

#[test]
fn a_program_that_cannot_be_started_fails_the_run_with_the_reason() {
    let f = Fixture::new();
    let runner = FakeRunner::default();
    *lock(&runner.fail) = Some(io::ErrorKind::NotFound);
    let rig = Rig::with(runner);
    let run = rig.start(&f).unwrap();
    let done = rig.wait_for(&run.id, Status::Failed);
    let message = done.run.message.unwrap();
    assert!(message.contains("couldn't be started") && message.contains("Settings"), "{message}");
    assert!(done.output.contains("✗"), "{}", done.output);
}

#[test]
fn how_a_run_ended_follows_from_what_the_process_did() {
    let code = |c: i32| -> io::Result<Exit> { Ok(Exit { code: Some(c), signal: None }) };
    let bad = ResultInfo { is_error: true, text: "\n  Out of credit\nmore".into(), ..Default::default() };
    let fine = ResultInfo::default();

    assert_eq!(runs::outcome(false, &code(0), Some(&fine), false), (Status::Finished, None, Some(0)));
    assert_eq!(runs::outcome(false, &code(0), None, false), (Status::Finished, None, Some(0)), "no result line at all is still a clean exit");
    assert_eq!(runs::outcome(false, &code(0), Some(&bad), false), (Status::Failed, Some("Out of credit".into()), Some(0)));
    let (status, message, exit) = runs::outcome(false, &code(1), Some(&bad), false);
    assert_eq!((status, exit), (Status::Failed, Some(1)));
    assert_eq!(message.unwrap(), "Claude Code stopped with exit code 1: Out of credit");
    let (_, message, _) = runs::outcome(false, &Ok(Exit { code: None, signal: Some(9) }), None, false);
    assert_eq!(message.unwrap(), "Claude Code stopped with signal 9");
    let (status, message, exit) = runs::outcome(true, &code(1), Some(&bad), false);
    assert_eq!((status, exit), (Status::Cancelled, Some(1)), "a cancelled run is cancelled whatever it exited with");
    assert_eq!(message.unwrap(), "Stopped at your request");
    let (status, message, _) = runs::outcome(false, &Err(io::Error::other("lost it")), None, false);
    assert_eq!(status, Status::Failed);
    assert!(message.unwrap().contains("lost it"));
}

// ---- runs: cancel and limits ----------------------------------------------------------------------------

#[test]
fn cancel_stops_the_process_and_the_run_ends_cancelled() {
    let f = Fixture::new();
    let rig = Rig::new();
    let run = rig.start(&f).unwrap();
    let process = rig.runner.handle(0);
    process.stdout(INIT);
    wait_until("output", || rig.snapshot(&run.id).output.contains("Started"));

    let cancelling = rig.runs.cancel(&run.id).unwrap();
    assert!(cancelling.cancelling || cancelling.status == Status::Cancelled);
    let done = rig.wait_for(&run.id, Status::Cancelled);
    assert!(process.killed(), "the whole process tree was stopped");
    assert_eq!(done.run.message.as_deref(), Some("Stopped at your request"));
    assert!(done.run.finished_at.is_some() && !done.run.cancelling);
    assert!(done.output.contains("Started"), "what it had said stays");
    assert_eq!(*lock(&rig.host.vault_calls), vec![(Vec::<String>::new(), false)], "nothing is linked after a cancel, but the vault is read in");

    // Cancelling again is harmless, and the prompt can be run again.
    assert_eq!(rig.runs.cancel(&run.id).unwrap().status, Status::Cancelled);
    let again = rig.start(&f).unwrap();
    assert_ne!(again.id, run.id);
    rig.runs.cancel_all();
    let second = rig.runner.handle(1);
    wait_until("the second process to be stopped", || second.killed());
    assert!(rig.runs.cancel("no-such-run").is_err());
}

#[test]
fn a_prompt_runs_once_at_a_time_and_only_a_few_runs_go_at_once() {
    let f = Fixture::new();
    let rig = Rig::new();
    let first = rig.start(&f).unwrap();
    let err = rig.start(&f).unwrap_err().to_string();
    assert!(err.contains("already running"), "{err}");

    let extra = f.more_prompts(MAX_CONCURRENT);
    let mut ids = vec![first.id.clone()];
    for id in &extra[..MAX_CONCURRENT - 1] {
        ids.push(rig.start_with(&f, id, ClaudeSettings::default()).unwrap().id);
    }
    let err = rig.start_with(&f, &extra[MAX_CONCURRENT - 1], ClaudeSettings::default()).unwrap_err().to_string();
    assert!(err.contains(&format!("{MAX_CONCURRENT} runs are already going")), "{err}");
    assert!(rig.runner.spawned() <= MAX_CONCURRENT);

    // Room again once one is over.
    rig.runs.cancel(&first.id).unwrap();
    rig.wait_for(&first.id, Status::Cancelled);
    assert!(rig.start_with(&f, &extra[MAX_CONCURRENT - 1], ClaudeSettings::default()).is_ok());
    rig.runs.cancel_all();
    for id in ids {
        rig.wait_for(&id, Status::Cancelled);
    }
}

#[test]
fn once_a_restart_for_an_update_is_decided_no_run_can_start_until_it_is_called_off() {
    let f = Fixture::new();
    let rig = Rig::new();
    let run = rig.start(&f).unwrap();
    assert_eq!(rig.runs.running().len(), 1);

    // A run is going: the restart can't be held, and is told which.
    let going = rig.runs.hold_for_restart().unwrap_err();
    assert_eq!(going.iter().map(|r| r.prompt_title.as_str()).collect::<Vec<_>>(), ["Call summary"]);
    // Refusing the hold didn't stop new runs.
    let extra = f.more_prompts(1);
    let second = rig.start_with(&f, &extra[0], ClaudeSettings::default()).unwrap();

    rig.runs.cancel_all();
    rig.wait_for(&run.id, Status::Cancelled);
    rig.wait_for(&second.id, Status::Cancelled);
    assert!(rig.runs.running().is_empty());

    // Nothing going: held, and from then on nothing starts, so nothing can be cut off by the restart.
    rig.runs.hold_for_restart().unwrap();
    let spawned = rig.runner.spawned();
    let refused = rig.start(&f).unwrap_err().to_string();
    assert!(refused.contains("restarting to install an update"), "{refused}");
    assert_eq!(rig.runner.spawned(), spawned, "no process was started");
    assert!(rig.runs.running().is_empty());

    // The install failed after all: runs start again.
    rig.runs.release_hold();
    let after = rig.start(&f).unwrap();
    rig.runs.cancel_all();
    rig.wait_for(&after.id, Status::Cancelled);
}

#[test]
fn runs_are_listed_with_their_output_for_a_window_that_was_away() {
    let f = Fixture::new();
    let rig = Rig::new();
    let run = rig.start(&f).unwrap();
    rig.runner.handle(0).stdout("a line of output");
    wait_until("output", || rig.snapshot(&run.id).output.contains("a line of output"));
    let listed = rig.runs.list();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].run.id.as_str(), listed[0].run.prompt_id.as_str(), listed[0].run.prompt_title.as_str()), (run.id.as_str(), f.prompt.id.as_str(), "Call summary"));
    assert!(listed[0].output.contains("a line of output") && listed[0].run.chunks == 1);
    rig.runs.cancel_all();
}

#[test]
fn the_output_kept_for_the_screen_is_capped() {
    let f = Fixture::new();
    let rig = Rig::new();
    let run = rig.start(&f).unwrap();
    let process = rig.runner.handle(0);
    for i in 0..700 {
        process.stdout(&format!("line-{i} {}", "x".repeat(990)));
    }
    process.exit(0);
    let done = rig.wait_for(&run.id, Status::Finished);
    assert!(done.output.len() <= MAX_OUTPUT_BYTES + 100, "{} bytes", done.output.len());
    assert!(done.output.contains("The output is cut here"));
    assert!(!done.output.contains("line-699 "));
}

// ---- the note kept about each run, and secrets --------------------------------------------------------------

fn request_in(cwd: &str) -> RunRequest {
    RunRequest {
        project: "acme".into(),
        thread: "call".into(),
        prompt_id: "p1".into(),
        prompt_title: "Title".into(),
        run_id: String::new(),
        cwd: PathBuf::from(cwd),
        text: String::new(),
    }
}

fn step(name: &str, detail: &str) -> Step {
    Step { name: name.into(), detail: detail.into(), failed: false }
}

fn facts<'a>(status: &'a str, result: Option<&'a ResultInfo>, steps: &'a [Step], files: &'a [String]) -> RunFacts<'a> {
    RunFacts {
        id: "r1",
        status,
        message: None,
        model: "claude-test",
        permission_mode: "acceptEdits",
        started_at: "2026-11-02T22:59:36.958Z",
        finished_at: "2026-11-02T22:59:47.958Z",
        seconds: 11,
        result,
        steps,
        files,
    }
}

fn note_file(f: &Fixture, id: &str) -> PathBuf {
    f.project_dir().join("threads").join(&f.thread).join("runs").join(format!("{id}.md"))
}

fn sample_note(f: &Fixture, id: &str, prompt_id: &str) -> RunRecord {
    RunRecord {
        id: id.into(),
        project: f.project.clone(),
        thread: f.thread.clone(),
        prompt_id: prompt_id.into(),
        status: "finished".into(),
        model: "claude-test".into(),
        permission_mode: "acceptEdits".into(),
        started_at: "2026-11-02T22:59:36.958Z".into(),
        finished_at: "2026-11-02T22:59:47.958Z".into(),
        seconds: 11,
        turns: Some(2),
        files: vec!["summary.md".into()],
        body: "Wrote it.\n\n## Activity\n\n- Write `deliverables/summary.md`\n".into(),
    }
}

#[test]
fn a_finished_run_leaves_a_note_for_its_thread() {
    let f = Fixture::new();
    let rig = Rig::with(FakeRunner::scripted(&[INIT, SAID, RESULT_OK], &["a warning"], 0));
    // What the vault says it published and linked for the run is what the note lists as produced.
    *lock(&rig.host.outcome) = VaultOutcome { linked: vec!["summary@v0.2.md".into()], ..Default::default() };
    let run = rig.start(&f).unwrap();
    rig.wait_for(&run.id, Status::Finished);

    let kept = lock(&rig.host.records).clone();
    assert_eq!(kept.len(), 1);
    let note = &kept[0];
    assert_eq!(
        (note.id.as_str(), note.project.as_str(), note.thread.as_str(), note.prompt_id.as_str()),
        (run.id.as_str(), f.project.as_str(), f.thread.as_str(), f.prompt.id.as_str())
    );
    assert_eq!((note.status.as_str(), note.model.as_str(), note.permission_mode.as_str(), note.turns), ("finished", "claude-test", "acceptEdits", Some(2)));
    assert_eq!(note.files, ["summary@v0.2.md"]);
    assert!(note.finished_at >= note.started_at);
    assert!(note.body.starts_with("Wrote deliverables/summary.md\n"), "{}", note.body);
    assert!(note.body.contains("\n## Activity\n\n- Write `deliverables/summary.md`\n"), "{}", note.body);
    assert!(note.body.contains("- Refused by the permission mode: `Bash curl example.com`"), "{}", note.body);
    // Not the command line, not the text that was sent, not the raw output.
    assert!(!note.body.contains("--permission-mode") && !note.body.contains(&f.request().text) && !note.body.contains("a warning"), "{}", note.body);
}

#[test]
fn a_failed_stopped_or_unstartable_run_is_noted_too() {
    let f = Fixture::new();

    let rig = Rig::with(FakeRunner::scripted(&[INIT, SAID, RESULT_ERROR], &[], 0));
    let failed = rig.start(&f).unwrap();
    rig.wait_for(&failed.id, Status::Failed);
    let note = lock(&rig.host.records)[0].clone();
    assert_eq!((note.status.as_str(), note.turns), ("failed", Some(0)));
    assert!(note.body.starts_with("Failed: Invalid API key. Please run /login\n"), "{}", note.body);
    assert!(note.body.contains("- Write `deliverables/summary.md`"), "what it did before it failed is kept");

    let rig = Rig::new();
    let run = rig.start(&f).unwrap();
    rig.runner.handle(0).stdout(SAID);
    wait_until("the output", || rig.snapshot(&run.id).output.contains("→ Write"));
    rig.runs.cancel(&run.id).unwrap();
    rig.wait_for(&run.id, Status::Cancelled);
    let note = lock(&rig.host.records)[0].clone();
    assert_eq!(note.status, "cancelled");
    assert!(note.body.starts_with("Stopped at your request\n") && note.body.contains("- Write `deliverables/summary.md`"), "{}", note.body);

    let runner = FakeRunner::default();
    *lock(&runner.fail) = Some(io::ErrorKind::NotFound);
    let rig = Rig::with(runner);
    let run = rig.start(&f).unwrap();
    rig.wait_for(&run.id, Status::Failed);
    let note = lock(&rig.host.records)[0].clone();
    assert!(note.status == "failed" && note.body.starts_with("Failed: Claude Code couldn't be started"), "{}", note.body);
}

#[test]
fn a_note_that_cannot_be_kept_is_said_in_the_output_and_the_run_still_ends() {
    let f = Fixture::new();
    let rig = Rig::with(FakeRunner::scripted(&[SAID, RESULT_OK], &[], 0));
    *lock(&rig.host.record_error) = Some("the thread is gone".into());
    let run = rig.start(&f).unwrap();
    let done = rig.wait_for(&run.id, Status::Finished);
    assert!(done.output.contains("✗ The note about this run couldn't be kept in the thread: the thread is gone"), "{}", done.output);
    assert!(lock(&rig.host.records).is_empty());
}

#[test]
fn the_note_lists_what_was_done_with_paths_relative_to_the_project() {
    let result = ResultInfo { is_error: false, text: "Done.".into(), turns: Some(3), denials: vec![] };
    let produced = ["a@v0.2.md".to_string()];
    for cwd in [r"C:\Users\me\Ledger\projects\acme", "/home/me/Ledger/projects/acme"] {
        let sep = if cwd.contains('\\') { "\\" } else { "/" };
        let at = |rest: &str| format!("{cwd}{sep}{}", rest.replace('/', sep));
        let steps = vec![
            step("Read", &at("README.md")),
            step("Write", &at("deliverables/a.md")),
            step("Bash", "echo `hi`"),
            Step { name: String::new(), detail: format!("Permission denied: {}", at("x.md")), failed: true },
        ];
        let note = records::build(&request_in(cwd), &facts("finished", Some(&result), &steps, &produced), &Redactor::default());

        assert_eq!(note.files, ["a@v0.2.md"], "{cwd}: the note lists what the vault says the run produced");
        assert!(note.body.starts_with("Done.\n\n## Activity\n\n- Read `README.md`\n- Write `deliverables/a.md`\n"), "{cwd}: {}", note.body);
        assert!(note.body.contains("- Bash `echo 'hi'`\n- Failed: Permission denied: x.md\n"), "{cwd}: {}", note.body);
        assert!(!note.body.contains("Ledger") && !note.body.contains(cwd), "{cwd}: the note says nothing about where the vault is: {}", note.body);
        assert_eq!((note.turns, note.seconds, note.model.as_str()), (Some(3), 11, "claude-test"));
    }
}

#[test]
fn the_note_says_how_a_run_ended_and_stays_small() {
    let request = request_in("/p");
    let steps: Vec<Step> = (0..250).map(|i| step("Read", &format!("/p/f{i}.md"))).collect();

    let mut stopped = facts("failed", None, &steps, &[]);
    stopped.message = Some("Claude Code stopped with exit code 1");
    let note = records::build(&request, &stopped, &Redactor::default());
    assert!(note.body.starts_with("Failed: Claude Code stopped with exit code 1\n"), "{}", note.body);
    assert!(note.body.contains("- Read `f199.md`\n- …and 50 more\n") && !note.body.contains("f200.md"), "the list is capped");
    assert_eq!(note.turns, None);

    stopped.status = "cancelled";
    stopped.message = Some("Stopped at your request");
    assert!(records::build(&request, &stopped, &Redactor::default()).body.starts_with("Stopped at your request\n"));

    // Nothing said and nothing done: no empty list.
    let quiet = records::build(&request, &facts("finished", None, &[], &[]), &Redactor::default());
    assert_eq!(quiet.body, "Claude Code finished without a closing message.\n");

    let long = ResultInfo { is_error: false, text: "word ".repeat(10_000), turns: None, denials: vec![] };
    let note = records::build(&request, &facts("finished", Some(&long), &[], &[]), &Redactor::default());
    assert!(note.body.chars().count() < 20_100 && note.body.trim_end().ends_with('…'), "{} characters", note.body.chars().count());
}

#[test]
fn the_note_names_no_folder_on_this_machine_and_no_secrets() {
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let f = Fixture::with_body(&format!("Deploy with {token} please."));
    let ledger = f.root().join(".ledger");
    fs::create_dir_all(&ledger).unwrap();
    fs::write(ledger.join(".env"), "GITHUB_TOKEN=github_pat_11AAAAAAA0abcdefghijklmnopqrstuvwxyz\nLEDGER_ENCRYPTION_KEY=\"zzzzyyyyxxxxwwww\"\n").unwrap();

    let folder = f.request().cwd.display().to_string();
    let absolute = format!("{folder}{0}deliverables{0}summary.md", std::path::MAIN_SEPARATOR);
    let said = serde_json::json!({"type": "assistant", "message": {"content": [
        {"type": "tool_use", "name": "Write", "input": {"file_path": absolute}},
        {"type": "tool_use", "name": "Bash", "input": {"command": format!("echo `x` {token} zzzzyyyyxxxxwwww")}},
    ]}})
    .to_string();
    let result = serde_json::json!({"type": "result", "is_error": false, "num_turns": 1, "result": format!("Wrote {absolute}. The key is zzzzyyyyxxxxwwww, and {token}.")}).to_string();
    let rig = Rig::with(FakeRunner::scripted(&[&said, &result], &[], 0));
    let run = rig.start(&f).unwrap();
    rig.wait_for(&run.id, Status::Finished);

    let note = lock(&rig.host.records)[0].clone();
    assert!(!note.body.contains(&folder), "the folder the vault sits in: {}", note.body);
    assert!(note.body.contains("- Write `deliverables/summary.md`\n"), "{}", note.body);
    assert!(note.body.contains("- Bash `echo 'x' [redacted] [redacted]`\n"), "{}", note.body);
    for secret in [token, "zzzzyyyyxxxxwwww", "github_pat_11AAAAAAA0"] {
        assert!(!note.body.contains(secret), "{secret}: {}", note.body);
    }
}

#[test]
fn secrets_never_reach_the_screen() {
    let token = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let f = Fixture::with_body(&format!("Deploy with {token} please."));
    let ledger = f.root().join(".ledger");
    fs::create_dir_all(&ledger).unwrap();
    fs::write(ledger.join(".env"), "# secrets\nGITHUB_TOKEN=github_pat_11AAAAAAA0abcdefghijklmnopqrstuvwxyz\nLEDGER_ENCRYPTION_KEY=\"zzzzyyyyxxxxwwww\"\n").unwrap();

    let leak = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"the key is zzzzyyyyxxxxwwww and Authorization: Bearer abcdefghijklmnopqrstuvwx"}]}}"#;
    let rig = Rig::with(FakeRunner::scripted(
        &[leak, "export GITHUB_TOKEN=github_pat_11AAAAAAA0abcdefghijklmnopqrstuvwxyz", "ANTHROPIC_API_KEY=sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv", "all done on the main branch"],
        &["stderr echo zzzzyyyyxxxxwwww"],
        0,
    ));
    let run = rig.start(&f).unwrap();
    // What is sent is what was shown, untouched: that is the person's own text.
    assert!(String::from_utf8(rig.runner.spec(0).stdin).unwrap().contains(token));
    let done = rig.wait_for(&run.id, Status::Finished);

    for secret in [token, "zzzzyyyyxxxxwwww", "github_pat_11AAAAAAA0", "abcdefghijklmnopqrstuvwx", "sk-ant-api03-AbCd"] {
        assert!(!done.output.contains(secret), "{secret} on screen: {}", done.output);
        assert!(!rig.events().iter().any(|e| e.chunk.as_ref().is_some_and(|c| c.text.contains(secret))), "{secret} in an event");
    }
    assert!(done.output.contains("[redacted]"));
    assert!(done.output.contains("all done on the main branch"), "ordinary text is left alone");
}

#[test]
fn a_note_is_a_markdown_file_in_the_thread_that_survives_reading_the_vault_again() {
    let mut f = Fixture::new();
    let note = sample_note(&f, "1793660376958-4o6j4fvm", &f.prompt.id.clone());
    finish::keep_record(&mut f.core, &note).unwrap();

    let text = fs::read_to_string(note_file(&f, &note.id)).unwrap();
    assert!(text.starts_with("---\n"), "{text}");
    for expected in [
        "id: 1793660376958-4o6j4fvm".to_string(),
        format!("prompt: {}", f.prompt.id),
        "status: finished".to_string(),
        "model: claude-test".to_string(),
        "permissionMode: acceptEdits".to_string(),
        "startedAt: ".to_string(),
        "finishedAt: ".to_string(),
        "seconds: 11".to_string(),
        "turns: 2".to_string(),
        "deliverables:\n- summary.md".to_string(),
        "---\n\nWrote it.\n\n## Activity\n\n- Write `deliverables/summary.md`\n".to_string(),
    ] {
        assert!(text.contains(&expected), "{expected} is missing from:\n{text}");
    }
    assert!(!text.contains(&f.root().display().to_string()), "nothing about where the vault is");

    f.core.reload().unwrap();
    let prompt = prompts::get(&f.core.conn, &f.project, &f.thread, &f.prompt.id).unwrap();
    assert_eq!(prompt.runs.len(), 1);
    let run = &prompt.runs[0];
    assert_eq!((run.id.as_str(), run.status.as_str(), run.model.as_str(), run.seconds, run.turns), (note.id.as_str(), "finished", "claude-test", 11, Some(2)));
    assert_eq!((run.permission_mode.as_str(), run.started_at.as_str()), ("acceptEdits", "2026-11-02T22:59:36.958Z"));
    assert_eq!(run.files, ["summary.md"]);
    assert!(run.html.contains("<h2>Activity</h2>") && run.html.contains("<code>deliverables/summary.md</code>"), "{}", run.html);
    assert_eq!(fs::read_to_string(note_file(&f, &note.id)).unwrap(), text, "reading the vault again leaves the file as it was");
}

#[test]
fn the_runs_of_a_prompt_are_listed_newest_first_and_only_for_that_prompt() {
    let mut f = Fixture::new();
    let other = f.more_prompts(1).remove(0);
    let mut older = sample_note(&f, "1793660000000-aaaaaaaa", &f.prompt.id.clone());
    older.started_at = "2026-11-02T10:00:00.000Z".into();
    let mut newer = sample_note(&f, "1793660999999-bbbbbbbb", &f.prompt.id.clone());
    newer.started_at = "2026-11-02T11:00:00.000Z".into();
    for note in [&newer, &older, &sample_note(&f, "1793660555555-cccccccc", &other)] {
        finish::keep_record(&mut f.core, note).unwrap();
    }
    let ids = |prompt: &str| -> Vec<String> { prompts::get(&f.core.conn, &f.project, &f.thread, prompt).unwrap().runs.into_iter().map(|r| r.id).collect() };
    assert_eq!(ids(&f.prompt.id), [newer.id.as_str(), older.id.as_str()]);
    assert_eq!(ids(&other), ["1793660555555-cccccccc"]);
    let thread = threads::get(&f.core.conn, &f.root(), &f.project, &f.thread).unwrap();
    assert_eq!(thread.prompts.iter().map(|p| p.runs.len()).sum::<usize>(), 3, "the thread's prompts carry their runs");
}

#[test]
fn deleting_a_thread_takes_its_notes_with_it_and_deleting_a_prompt_keeps_them() {
    let mut f = Fixture::new();
    let note = sample_note(&f, "1793660376958-4o6j4fvm", &f.prompt.id.clone());
    finish::keep_record(&mut f.core, &note).unwrap();

    prompts::delete(&f.core.conn, &f.project, &f.thread, &f.prompt.id).unwrap();
    f.persist();
    f.core.reload().unwrap();
    assert!(note_file(&f, &note.id).is_file(), "the note outlives the prompt it was about");

    threads::delete(&f.core.conn, &f.root(), &f.project, &f.thread).unwrap();
    f.persist();
    assert!(!note_file(&f, &note.id).exists());
    assert!(!f.project_dir().join("threads").join(&f.thread).exists(), "no folder is left to bring the thread back when the vault is read");
    f.core.reload().unwrap();
    assert!(threads::require(&f.core.conn, &f.project, &f.thread).is_err());
    assert!(finish::keep_record(&mut f.core, &note).is_err(), "a note can't be kept for a thread that is gone");
}

#[test]
fn a_note_edited_by_hand_is_read_as_it_is_written() {
    let mut f = Fixture::new();
    let dir = f.project_dir().join("threads").join(&f.thread).join("runs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("1793660376958-dddddddd.md"), format!("---\nprompt: {}\nstatus: maybe\n---\n\nHand written.\n", f.prompt.id)).unwrap();
    fs::write(dir.join("no-prompt.md"), "---\nstatus: finished\n---\n\nThis one says nothing about a prompt.\n").unwrap();
    f.core.reload().unwrap();

    let runs = prompts::get(&f.core.conn, &f.project, &f.thread, &f.prompt.id).unwrap().runs;
    assert_eq!(runs.len(), 1, "a file without a prompt isn't a note");
    assert_eq!((runs[0].id.as_str(), runs[0].status.as_str()), ("1793660376958-dddddddd", "failed"), "a status this version doesn't know is not shown as a success");
    assert!(runs[0].html.contains("Hand written."));
    assert!(fs::read_to_string(dir.join("no-prompt.md")).unwrap().contains("says nothing"), "the file is left alone");
}

#[test]
fn the_redactor_knows_the_vaults_values_and_the_shapes_of_tokens() {
    let known = Redactor::new(["supersecretvalue123".to_string(), "short".to_string()]);
    assert_eq!(known.apply("the key is supersecretvalue123, ok"), "the key is [redacted], ok");
    assert_eq!(known.apply("a short word"), "a short word", "values too short to be secrets are not masked");

    let shapes = Redactor::default();
    for (input, gone) in [
        ("ghp_abcdefghijklmnopqrstuvwxyz0123456789", "abcdefghij"),
        ("token github_pat_11ABCDEFG0abcdefghijklmnop here", "11ABCDEFG0"),
        ("Authorization: Bearer abcdefghijklmnop1234567890", "abcdefghijklmnop"),
        ("authorization: bearer abcdefghijklmnop1234567890", "abcdefghijklmnop"),
        ("export GITHUB_TOKEN=whatever1234", "whatever1234"),
        ("DB_PASSWORD=\"hunter2hunter2\"", "hunter2"),
        ("url?access_token=abcd1234&x=1", "abcd1234"),
        ("sk-ant-api03-AbCdEfGhIjKlMnOp", "AbCdEf"),
        ("ledger-key-1:AAAABBBBCCCCDDDD", "AAAABBBB"),
        ("AKIAABCDEFGHIJKLMNOP", "ABCDEFGH"),
    ] {
        let out = shapes.apply(input);
        assert!(!out.contains(gone) && out.contains("[redacted]"), "{input} -> {out}");
    }
    assert_eq!(shapes.apply("Authorization: Bearer abcdefghijklmnop1234567890"), "Authorization: Bearer [redacted]");
    assert_eq!(shapes.apply("GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789 done"), "GITHUB_TOKEN=[redacted] done");

    for fine in [
        "Wrote deliverables/task-summary.md on the main branch",
        "sk-learn is a library",
        "ask-me-anything-about-this-rather-long-title-here",
        "héllo wörld 日本語 🙂 = fine",
        "a = b == c",
        "KEY=",
        "the AKIA prefix alone",
    ] {
        assert_eq!(shapes.apply(fine), fine);
    }
}

#[test]
fn the_redactor_reads_the_vaults_env_file() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".ledger")).unwrap();
    fs::write(dir.path().join(".ledger").join(".env"), "# comment\nGITHUB_TOKEN=abcd1234efgh5678\nexport OTHER='quoted value 99'\nshort=abc\n").unwrap();
    let redactor = Redactor::for_vault(dir.path());
    assert_eq!(redactor.apply("a abcd1234efgh5678 b quoted value 99 c abc d"), "a [redacted] b [redacted] c abc d");
    assert_eq!(Redactor::for_vault(&dir.path().join("nowhere")).apply("fine"), "fine");
}

// ---- reading the output ---------------------------------------------------------------------------------

#[test]
fn streaming_json_is_shown_as_what_was_said_and_done() {
    let init = render::render_line(INIT).text;
    assert_eq!(init, "● Started: claude-test, permission mode acceptEdits\n");

    let said = render::render_line(SAID).text;
    assert_eq!(said, "I'll write the summary.\n→ Write deliverables/summary.md\n");

    // File contents and command output are not copied into the output; errors are.
    let ok = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"1","content":"file contents SECRET"}]}}"#;
    assert_eq!(render::render_line(ok).text, "");
    let bad = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"1","is_error":true,"content":[{"type":"text","text":"Permission denied: Write\nmore"}]}]}}"#;
    assert_eq!(render::render_line(bad).text, "  ✗ Permission denied: Write\n");

    // What the note about the run is made from: the model, the tools used, and the calls that failed.
    assert_eq!(render::render_line(INIT).model.as_deref(), Some("claude-test"));
    assert_eq!(render::render_line(SAID).steps, [step("Write", "deliverables/summary.md")]);
    assert_eq!(render::render_line(bad).steps, [Step { name: String::new(), detail: "Permission denied: Write".into(), failed: true }]);
    assert!(render::render_line(ok).steps.is_empty() && render::render_line("Error: something broke").steps.is_empty());

    let result = render::render_line(RESULT_OK);
    assert_eq!(result.text, "");
    let info = result.result.unwrap();
    assert_eq!((info.is_error, info.turns), (false, Some(2)));
    assert_eq!(info.denials, ["Bash curl example.com"]);

    assert_eq!(render::render_line("Error: something broke").text, "Error: something broke\n", "a line that isn't JSON is shown as it is");
    assert_eq!(render::render_line("{\"no\": \"type\"}").text, "{\"no\": \"type\"}\n");
    assert_eq!(render::render_line("   ").text, "");
    assert_eq!(render::render_line(r#"{"type":"stream_event","event":{}}"#).text, "");
    let retry = render::render_line(r#"{"type":"system","subtype":"api_retry","attempt":2,"error":"rate_limit"}"#).text;
    assert!(retry.contains("rate_limit") && retry.contains("attempt 2"), "{retry}");

    let closing = render::summary(Some(&info), 65);
    assert!(closing.contains("— 1:05 · 2 turns\n") && closing.contains("Refused by the permission mode: Bash curl example.com"), "{closing}");
    assert!(!closing.contains('$'), "{closing}");
    assert_eq!(render::summary(None, 7), "— 0:07\n");
}

// ---- the vault after a run ------------------------------------------------------------------------------

/// The part of a run that follows the process: see what changed in the deliverables since `baseline`, and put the
/// vault in order.
fn after_run(f: &mut Fixture, baseline: &Baseline, link: bool) -> VaultOutcome {
    let mut request = f.request();
    request.run_id = "run-1".into();
    let changes = baseline.changes(&request.cwd.join("deliverables"));
    finish::refresh_vault(&mut f.core, &request, &changes, link)
}

fn deliverables_folder(f: &Fixture) -> PathBuf {
    let dir = f.project_dir().join("deliverables");
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A deliverable the person wrote and published once, as 0.1.
fn seed_published(f: &mut Fixture, name: &str, text: &str) {
    fs::write(deliverables_folder(f).join(name), text).unwrap();
    f.core.reload().unwrap();
    deliverables::publish(&f.core.conn, &f.root(), &f.project, name, Bump::Minor, None).unwrap();
    f.persist();
}

fn linked_paths(f: &Fixture) -> Vec<String> {
    prompts::get(&f.core.conn, &f.project, &f.thread, &f.prompt.id).unwrap().deliverables.into_iter().map(|d| d.path).collect()
}

fn body_of(f: &Fixture, path: &str) -> String {
    deliverables::get(&f.core.conn, &f.project, path).unwrap().body.unwrap()
}

fn versions(f: &Fixture, base: &str) -> Vec<String> {
    deliverables::versions_of(&f.core.conn, &f.project, base).unwrap()
}

fn version_posts(f: &Fixture) -> Vec<String> {
    threads::get(&f.core.conn, &f.root(), &f.project, &f.thread).unwrap().posts.into_iter().filter(|p| p.version_published).map(|p| p.body).collect()
}

#[test]
fn a_new_file_is_published_as_the_first_version_and_that_version_is_linked() {
    let mut f = Fixture::new();
    let dir = deliverables_folder(&f);
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("summary.md"), "# Summary\n\nAna wants SSO.\n").unwrap();
    fs::write(dir.join("chart.png"), b"\x89PNG").unwrap();

    let outcome = after_run(&mut f, &baseline, true);
    assert!(outcome.reloaded && outcome.error.is_none(), "{outcome:?}");
    assert_eq!(outcome.linked, ["chart@v0.1.png", "summary@v0.1.md"]);
    assert!(outcome.kept.is_empty());

    assert_eq!(linked_paths(&f), ["chart@v0.1.png", "summary@v0.1.md"], "the versions are linked, not the live files");
    assert!(body_of(&f, "summary@v0.1.md").contains("Ana wants SSO"));
    assert!(body_of(&f, "summary.md").contains("Ana wants SSO"), "the live file stays");
    assert_eq!(fs::read(dir.join("chart@v0.1.png")).unwrap(), b"\x89PNG", "a file that isn't text is copied");

    // Each version is noted in the thread, and the deliverable knows the prompt that produced it.
    let posts = version_posts(&f);
    assert_eq!(posts.len(), 2);
    assert!(posts.iter().any(|p| p.contains("Version 0.1 of **summary.md** was published from the prompt **Call summary**.")), "{posts:?}");
    assert_eq!(deliverables::get(&f.core.conn, &f.project, "summary.md").unwrap().prompts.len(), 1);
    let listed = deliverables::list(&f.core.conn, &f.project).unwrap();
    assert_eq!(listed.iter().find(|d| d.path == "summary.md").unwrap().prompt_count, 1);

    // Written to the vault's files, so it survives a reload.
    f.core.reload().unwrap();
    assert_eq!(linked_paths(&f), ["chart@v0.1.png", "summary@v0.1.md"]);
    assert!(dir.join("summary@v0.1.md").is_file());
}

#[test]
fn an_edited_file_with_nothing_unpublished_is_published_as_the_next_minor_and_that_is_linked() {
    let mut f = Fixture::new();
    seed_published(&mut f, "plan.md", "# Plan\n\nfirst text\n");
    let dir = deliverables_folder(&f);
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("plan.md"), "# Plan\n\nsecond text\n").unwrap();

    let outcome = after_run(&mut f, &baseline, true);
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert_eq!(outcome.linked, ["plan@v0.2.md"]);
    assert!(outcome.kept.is_empty(), "there was nothing to keep");
    assert_eq!(versions(&f, "plan.md"), ["0.1", "0.2"]);
    assert!(body_of(&f, "plan@v0.1.md").contains("first text") && body_of(&f, "plan@v0.2.md").contains("second text"));
    assert_eq!(linked_paths(&f), ["plan@v0.2.md"]);
}

#[test]
fn an_edited_file_with_unpublished_edits_has_them_published_first_and_only_the_runs_version_is_linked() {
    let mut f = Fixture::new();
    seed_published(&mut f, "plan.md", "# Plan\n\nfirst text\n");
    let dir = deliverables_folder(&f);
    // The person edited the live file after publishing: no version holds this.
    fs::write(dir.join("plan.md"), "# Plan\n\nthe person's edit\n").unwrap();
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("plan.md"), "# Plan\n\nthe run's edit\n").unwrap();

    let outcome = after_run(&mut f, &baseline, true);
    assert!(outcome.error.is_none(), "{outcome:?}");
    assert_eq!(outcome.kept, ["plan@v0.2.md"]);
    assert_eq!(outcome.linked, ["plan@v0.3.md"]);
    assert_eq!(versions(&f, "plan.md"), ["0.1", "0.2", "0.3"]);
    assert!(body_of(&f, "plan@v0.2.md").contains("the person's edit"), "what the file said before the run changed it");
    assert!(body_of(&f, "plan@v0.3.md").contains("the run's edit") && body_of(&f, "plan.md").contains("the run's edit"));
    assert_eq!(linked_paths(&f), ["plan@v0.3.md"], "the version made to keep the earlier edits is not linked");
    assert_eq!(version_posts(&f).len(), 2, "both are noted in the thread");

    f.core.reload().unwrap();
    assert!(body_of(&f, "plan@v0.2.md").contains("the person's edit"), "and both are in the vault's files");
}

#[test]
fn a_runs_version_is_recorded_as_ai_with_its_prompt_and_run_and_the_note_links_the_prompt() {
    let mut f = Fixture::new();
    let dir = deliverables_folder(&f);
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("summary.md"), "# Summary\n\nAna wants SSO.\n").unwrap();
    fs::write(dir.join("chart.png"), b"\x89PNG").unwrap();
    after_run(&mut f, &baseline, true);

    for base in ["summary.md", "chart.png"] {
        let known = provenance::resolve(&f.core.conn, &f.project, base).unwrap();
        assert_eq!(known.len(), 1);
        let v = &known[0];
        assert_eq!((v.author, v.thread.as_deref(), v.prompt.as_deref(), v.run.as_deref()), ("ai", Some(f.thread.as_str()), Some(f.prompt.id.as_str()), Some("run-1")), "{base}");
        assert!(v.record.is_some() && v.sha256.is_some(), "{base}");
    }
    let note = threads::get(&f.core.conn, &f.root(), &f.project, &f.thread).unwrap().posts.into_iter().find(|p| p.body.contains("**summary.md**")).unwrap();
    assert_eq!((note.prompt_id.as_deref(), note.prompt_title.as_deref()), (Some(f.prompt.id.as_str()), Some("Call summary")));

    // The link is in the vault's files, not only in memory.
    f.persist();
    f.core.reload().unwrap();
    let again = provenance::resolve(&f.core.conn, &f.project, "summary.md").unwrap();
    assert_eq!((again[0].author, again[0].run.as_deref(), again[0].prompt.as_deref()), ("ai", Some("run-1"), Some(f.prompt.id.as_str())));
}

#[test]
fn edits_kept_before_a_run_are_recorded_as_the_persons_and_not_linked_to_the_prompt() {
    let mut f = Fixture::new();
    seed_published(&mut f, "plan.md", "# Plan\n\nfirst text\n");
    let dir = deliverables_folder(&f);
    fs::write(dir.join("plan.md"), "# Plan\n\nthe person's edit\n").unwrap();
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("plan.md"), "# Plan\n\nthe run's edit\n").unwrap();
    after_run(&mut f, &baseline, true);

    let known = provenance::resolve(&f.core.conn, &f.project, "plan.md").unwrap();
    assert_eq!(known.iter().map(|v| (v.version.as_str(), v.author)).collect::<Vec<_>>(), [("0.1", "human"), ("0.2", "human"), ("0.3", "ai")]);
    assert_eq!((known[1].prompt.as_deref(), known[1].run.as_deref(), known[1].thread.as_deref()), (None, None, Some(f.thread.as_str())));
    assert_eq!(known[2].run.as_deref(), Some("run-1"));
    let notes = version_posts(&f);
    assert!(notes.iter().any(|n| n == "Version 0.2 of **plan.md** keeps your earlier edits, published just before the prompt **Call summary** ran."), "{notes:?}");
    assert!(notes.iter().any(|n| n == "Version 0.3 of **plan.md** was published from the prompt **Call summary**."), "{notes:?}");
}

#[test]
fn what_a_stopped_run_left_in_the_file_is_kept_as_that_runs_when_the_next_run_starts() {
    let mut f = Fixture::new();
    seed_published(&mut f, "plan.md", "# Plan\n\nfirst text\n");
    let dir = deliverables_folder(&f);

    // A run that is stopped halfway: it changed the file, nothing is published, and its note lists the file.
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("plan.md"), "# Plan\n\nhalf of the run's text\n").unwrap();
    after_run(&mut f, &baseline, false);
    let at = ids::now();
    records::insert(
        &f.core.conn,
        &RunRecord {
            id: "run-stopped".into(),
            project: f.project.clone(),
            thread: f.thread.clone(),
            prompt_id: f.prompt.id.clone(),
            status: "cancelled".into(),
            model: String::new(),
            permission_mode: String::new(),
            started_at: at.clone(),
            finished_at: at,
            seconds: 1,
            turns: None,
            files: vec!["plan.md".into()],
            body: "Stopped at your request".into(),
        },
    )
    .unwrap();
    f.persist();
    assert_eq!(provenance::stopped_run(&f.core.conn, &f.project, "plan.md").unwrap().map(|r| r.id), Some("run-stopped".to_string()));

    // The next run starts with that text in the file and no version holding it.
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("plan.md"), "# Plan\n\nthe next run's text\n").unwrap();
    let outcome = after_run(&mut f, &baseline, true);
    assert_eq!((outcome.kept.as_slice(), outcome.linked.as_slice()), (&["plan@v0.2.md".to_string()][..], &["plan@v0.3.md".to_string()][..]), "{outcome:?}");

    let known = provenance::resolve(&f.core.conn, &f.project, "plan.md").unwrap();
    assert_eq!((known[1].author, known[1].run.as_deref(), known[1].prompt.as_deref()), ("ai", Some("run-stopped"), Some(f.prompt.id.as_str())), "not the person's");
    assert_eq!((known[2].author, known[2].run.as_deref()), ("ai", Some("run-1")));
    assert!(
        version_posts(&f).iter().any(|n| n == "Version 0.2 of **plan.md** keeps what an earlier run of the prompt **Call summary** left in the file, published just before the prompt **Call summary** ran."),
        "{:?}",
        version_posts(&f)
    );
    assert!(provenance::stopped_run(&f.core.conn, &f.project, "plan.md").unwrap().is_none(), "newer versions hold it now");
}

#[test]
fn a_file_that_was_never_published_has_what_it_said_published_first() {
    let mut f = Fixture::new();
    let dir = deliverables_folder(&f);
    fs::write(dir.join("draft.md"), "# Draft\n\nnotes the person wrote\n").unwrap();
    f.core.reload().unwrap();
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("draft.md"), "# Draft\n\nthe run's text\n").unwrap();

    let outcome = after_run(&mut f, &baseline, true);
    assert_eq!((outcome.kept.as_slice(), outcome.linked.as_slice()), (&["draft@v0.1.md".to_string()][..], &["draft@v0.2.md".to_string()][..]), "{outcome:?}");
    assert!(body_of(&f, "draft@v0.1.md").contains("notes the person wrote") && body_of(&f, "draft@v0.2.md").contains("the run's text"));
    assert_eq!(linked_paths(&f), ["draft@v0.2.md"]);
}

#[test]
fn a_placeholder_counts_as_new_so_no_stub_is_published() {
    let mut f = Fixture::new();
    // "Create deliverable" makes a file with a title in it, and links it to the prompt.
    deliverables::create(&f.core.conn, &f.project, "Open hello world", "").unwrap();
    prompts::link_deliverable(&f.core.conn, &f.project, &f.thread, &f.prompt.id, "open-hello-world.md").unwrap();
    f.persist();
    let dir = deliverables_folder(&f);
    let baseline = Baseline::capture(&dir);
    fs::write(dir.join("open-hello-world.md"), "# Open hello world\n\nTwo sentences.\n").unwrap();

    let outcome = after_run(&mut f, &baseline, true);
    assert!(outcome.kept.is_empty(), "{outcome:?}");
    assert_eq!(outcome.linked, ["open-hello-world@v0.1.md"]);
    assert_eq!(versions(&f, "open-hello-world.md"), ["0.1"]);
    assert!(body_of(&f, "open-hello-world@v0.1.md").contains("Two sentences."));
    assert_eq!(linked_paths(&f), ["open-hello-world.md", "open-hello-world@v0.1.md"], "the link the prompt already had stays");
}

#[test]
fn a_run_that_did_not_finish_keeps_unpublished_edits_but_publishes_and_links_nothing_else() {
    let mut f = Fixture::new();
    seed_published(&mut f, "plan.md", "# Plan\n\nfirst text\n");
    seed_published(&mut f, "other.md", "# Other\n\nfirst text\n");
    let dir = deliverables_folder(&f);
    fs::write(dir.join("plan.md"), "# Plan\n\nthe person's edit\n").unwrap();
    let baseline = Baseline::capture(&dir);
    for name in ["plan.md", "other.md", "new.md"] {
        fs::write(dir.join(name), "# Half\n\nhalf done\n").unwrap();
    }

    let outcome = after_run(&mut f, &baseline, false);
    assert!(outcome.error.is_none() && outcome.linked.is_empty(), "{outcome:?}");
    assert_eq!(outcome.kept, ["plan@v0.2.md"], "the person's edits would otherwise be lost under the run's");
    assert!(body_of(&f, "plan@v0.2.md").contains("the person's edit"));
    assert_eq!(versions(&f, "other.md"), ["0.1"], "a file with nothing unpublished loses nothing, so nothing is published");
    assert!(deliverables::exists(&f.core.conn, &f.project, "new.md").unwrap() && versions(&f, "new.md").is_empty());
    assert!(linked_paths(&f).is_empty());
}

#[test]
fn a_deliverable_the_run_did_not_change_publishes_nothing_and_an_untouched_vault_is_not_reloaded() {
    let mut f = Fixture::new();
    seed_published(&mut f, "plan.md", "# Plan\n\nfirst text\n");
    let dir = deliverables_folder(&f);
    let baseline = Baseline::capture(&dir);

    let quiet = after_run(&mut f, &baseline, true);
    assert!(!quiet.reloaded && quiet.linked.is_empty() && quiet.kept.is_empty() && quiet.error.is_none(), "{quiet:?}");

    // Written again with what it said: the vault notices the file was touched and reads it in, but it isn't a change.
    fs::write(dir.join("plan.md"), "# Plan\n\nfirst text\n").unwrap();
    let rewritten = after_run(&mut f, &baseline, true);
    assert!(rewritten.linked.is_empty() && rewritten.kept.is_empty() && rewritten.error.is_none(), "{rewritten:?}");
    assert_eq!(versions(&f, "plan.md"), ["0.1"]);
    assert!(linked_paths(&f).is_empty());
}

#[test]
fn a_prompt_that_linked_two_versions_of_a_file_counts_once_for_it() {
    let mut f = Fixture::new();
    seed_published(&mut f, "plan.md", "# Plan\n\nfirst text\n");
    let dir = deliverables_folder(&f);
    for text in ["second text", "third text"] {
        let baseline = Baseline::capture(&dir);
        fs::write(dir.join("plan.md"), format!("# Plan\n\n{text}\n")).unwrap();
        after_run(&mut f, &baseline, true);
    }
    assert_eq!(linked_paths(&f), ["plan@v0.2.md", "plan@v0.3.md"]);
    assert_eq!(deliverables::get(&f.core.conn, &f.project, "plan.md").unwrap().prompts.len(), 1);
    assert_eq!(deliverables::list(&f.core.conn, &f.project).unwrap().iter().find(|d| d.path == "plan.md").unwrap().prompt_count, 1);

    // Publishing by hand afterwards still finds the prompt, and notes it in that prompt's thread.
    deliverables::publish(&f.core.conn, &f.root(), &f.project, "plan.md", Bump::Minor, None).unwrap();
    assert!(version_posts(&f).iter().any(|p| p.contains("Version 0.4 of")), "{:?}", version_posts(&f));
}

#[test]
fn a_run_that_creates_a_deliverable_ends_with_its_first_version_linked_to_the_prompt() {
    // The whole path, with the vault as the host: run, write a file as the program would, finish.
    struct VaultHost {
        core: Mutex<Core>,
        events: Mutex<Vec<RunEvent>>,
    }
    impl Host for VaultHost {
        fn emit(&self, event: &RunEvent) {
            lock(&self.events).push(event.clone());
        }
        fn vault_after_run(&self, request: &RunRequest, changes: &[Change], link: bool) -> VaultOutcome {
            finish::refresh_vault(&mut lock(&self.core), request, changes, link)
        }
        fn keep_record(&self, record: &RunRecord) -> std::result::Result<(), String> {
            finish::keep_record(&mut lock(&self.core), record).map_err(|e| e.to_string())
        }
    }

    let f = Fixture::new();
    let request = f.request();
    let (project, thread, prompt_id, root) = (f.project.clone(), f.thread.clone(), f.prompt.id.clone(), f.root());
    let Fixture { _dir, _config, core, .. } = f;
    let host = Arc::new(VaultHost { core: Mutex::new(core), events: Mutex::new(Vec::new()) });
    let runner = Arc::new(FakeRunner::default());
    let runs = Runs::new(runner.clone(), host.clone());
    let (cli_dir, search) = fake_cli();

    let run = runs.start(request.clone(), RunContext { root, settings: ClaudeSettings::default() }, &search).unwrap();
    let process = runner.handle(0);
    fs::write(request.cwd.join("deliverables").join("summary.md"), "# Summary\n").unwrap();
    process.stdout(SAID);
    process.stdout(RESULT_OK);
    process.exit(0);
    wait_until("the run to finish", || runs.snapshot(&run.id).unwrap().run.status == Status::Finished);

    let done = runs.snapshot(&run.id).unwrap();
    assert_eq!(done.run.linked, ["summary@v0.1.md"]);
    assert!(done.run.vault_reloaded);
    assert!(done.output.contains("Published, and linked to the prompt: summary@v0.1.md"), "{}", done.output);
    let core = lock(&host.core);
    let linked = prompts::get(&core.conn, &project, &thread, &prompt_id).unwrap().deliverables;
    assert_eq!(linked.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(), ["summary@v0.1.md"]);

    // And the thread has its note about the run, as a file and on the prompt, by the same id the run had.
    let noted = prompts::get(&core.conn, &project, &thread, &prompt_id).unwrap().runs;
    assert_eq!(noted.len(), 1);
    assert_eq!((noted[0].id.as_str(), noted[0].status.as_str(), noted[0].turns), (run.id.as_str(), "finished", Some(2)));
    assert_eq!(noted[0].files, ["summary@v0.1.md"]);
    let file = request.cwd.join("threads").join(&thread).join("runs").join(format!("{}.md", run.id));
    assert!(fs::read_to_string(file).unwrap().contains("Wrote deliverables/summary.md"));
    drop(core);
    drop(cli_dir);
}

// ---- real processes -------------------------------------------------------------------------------------
//
// These start this test binary itself, told by an environment variable what to do, so they need no other program
// and work the same everywhere. (Without the variable, `helper_process` does nothing.)

const HELPER_ENV: &str = "LEDGER_CLAUDE_TEST_HELPER";
const PIDFILE_ENV: &str = "LEDGER_CLAUDE_TEST_PIDFILE";

fn hide(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = command;
}

#[test]
fn helper_process() {
    match std::env::var(HELPER_ENV).as_deref() {
        Ok("echo") => {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input).unwrap();
            println!("<<{input}>>");
        }
        Ok("sleep") => thread::sleep(Duration::from_secs(120)),
        Ok("tree") => {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command.args(["--exact", "tests_claude_code::helper_process", "--nocapture"]).env(HELPER_ENV, "sleep");
            hide(&mut command);
            let mut child = command.spawn().unwrap();
            fs::write(std::env::var(PIDFILE_ENV).unwrap(), child.id().to_string()).unwrap();
            println!("ready");
            io::stdout().flush().unwrap();
            let _ = child.wait();
        }
        _ => {}
    }
}

fn helper_spec(role: &str, stdin: &str, cwd: &Path) -> SpawnSpec {
    SpawnSpec {
        program: std::env::current_exe().unwrap(),
        args: os(&["--exact", "tests_claude_code::helper_process", "--nocapture"]),
        cwd: cwd.to_path_buf(),
        env: vec![(HELPER_ENV.into(), role.into())],
        stdin: stdin.as_bytes().to_vec(),
    }
}

fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        // SAFETY: signal 0 only asks whether the process exists.
        unsafe { kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        let mut command = Command::new("tasklist");
        command.args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"]);
        hide(&mut command);
        String::from_utf8_lossy(&command.output().unwrap().stdout).contains(&format!("\"{pid}\""))
    }
}

#[test]
fn a_real_process_gets_the_prompt_on_standard_input_and_its_output_comes_back() {
    let dir = tempdir().unwrap();
    let mut spawned = OsRunner.spawn(&helper_spec("echo", HOSTILE, dir.path())).unwrap();
    let mut out = String::new();
    spawned.stdout.read_to_string(&mut out).unwrap();
    let exit = spawned.process.wait().unwrap();
    assert!(exit.success(), "{exit:?}");
    assert!(out.contains(&format!("<<{HOSTILE}>>")), "{out}");
}

#[test]
fn cancelling_a_real_process_stops_the_processes_it_started_too() {
    let dir = tempdir().unwrap();
    let pidfile = dir.path().join("child.pid");
    let mut spec = helper_spec("tree", "", dir.path());
    spec.env.push((PIDFILE_ENV.into(), pidfile.clone().into()));
    let spawned = OsRunner.spawn(&spec).unwrap();

    let mut lines = BufReader::new(spawned.stdout);
    let mut line = String::new();
    loop {
        line.clear();
        assert!(lines.read_line(&mut line).unwrap() > 0, "the helper ended before it was ready");
        if line.trim() == "ready" {
            break;
        }
    }
    let child: u32 = fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    assert!(alive(child), "the helper's own child is running");

    spawned.process.kill_tree();
    let process = spawned.process.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(process.wait());
    });
    let exit = rx.recv_timeout(Duration::from_secs(20)).expect("the process ended after being stopped").unwrap();
    assert!(!exit.success(), "{exit:?}");
    wait_until("the grandchild to be gone", || !alive(child));
    spawned.process.kill_tree(); // once it is over, stopping it again does nothing
}

#[test]
fn a_program_that_isnt_there_is_a_start_error_not_a_hang() {
    let dir = tempdir().unwrap();
    let mut spec = helper_spec("echo", "", dir.path());
    spec.program = dir.path().join("no-such-program");
    assert_eq!(OsRunner.spawn(&spec).err().map(|e| e.kind()), Some(io::ErrorKind::NotFound));
}

#[test]
fn a_command_runs_to_its_end_with_its_output_collected() {
    let dir = tempdir().unwrap();
    let captured = runner::capture(&OsRunner, &helper_spec("echo", "ping", dir.path()), Duration::from_secs(30)).unwrap();
    assert!(captured.exit.success() && !captured.timed_out && captured.stdout.contains("<<ping>>"), "{}", captured.stdout);
    let slow = runner::capture(&OsRunner, &helper_spec("sleep", "", dir.path()), Duration::from_millis(200)).unwrap();
    assert!(slow.timed_out && !slow.exit.success());
}

#[cfg(windows)]
#[test]
fn an_npm_style_cmd_shim_is_run_with_escaped_arguments_and_gets_the_prompt() {
    let dir = tempdir().unwrap();
    let shim = dir.path().join("claude.cmd");
    fs::write(&shim, "@echo off\r\necho ARGS:%*\r\nfindstr \"^\"\r\n").unwrap();
    let guard = dir.path().join("a folder").join("guard.json");
    let settings = ClaudeSettings { model: "sonnet[1m]".into(), ..Default::default() };
    let stdin = "hello & %PATH% \"quoted\" ^ | > <\nsecond line\n";
    let spec = SpawnSpec { program: shim, args: cli::run_args(&settings, Some(&guard)), cwd: dir.path().to_path_buf(), env: vec![], stdin: stdin.as_bytes().to_vec() };

    let mut spawned = OsRunner.spawn(&spec).unwrap();
    let mut out = String::new();
    spawned.stdout.read_to_string(&mut out).unwrap();
    assert!(spawned.process.wait().unwrap().success(), "{out}");
    // Rust quotes every argument it hands to a batch file, so the model comes through as one argument.
    assert!(out.contains("ARGS:-p --output-format stream-json --verbose --permission-mode acceptEdits --model \"sonnet[1m]\""), "{out}");
    assert!(out.contains("--strict-mcp-config"), "{out}");
    assert!(out.contains("guard.json"), "{out}");
    assert!(out.contains("hello & %PATH% \"quoted\" ^ | > <") && out.contains("second line"), "{out}");
}

#[cfg(unix)]
#[test]
fn a_script_standing_in_for_claude_is_started_directly_and_gets_the_prompt() {
    let dir = tempdir().unwrap();
    let script = dir.path().join("claude");
    fs::write(&script, "#!/bin/sh\necho \"ARGS:$*\"\ncat\n").unwrap();
    make_executable(&script);
    let spec = SpawnSpec {
        program: script,
        args: cli::run_args(&ClaudeSettings { confine: false, ..Default::default() }, None),
        cwd: dir.path().to_path_buf(),
        env: vec![],
        stdin: HOSTILE.as_bytes().to_vec(),
    };
    let mut spawned = OsRunner.spawn(&spec).unwrap();
    let mut out = String::new();
    spawned.stdout.read_to_string(&mut out).unwrap();
    assert!(spawned.process.wait().unwrap().success());
    assert!(out.contains("ARGS:-p --output-format stream-json --verbose --permission-mode acceptEdits") && out.contains(HOSTILE), "{out}");
}
