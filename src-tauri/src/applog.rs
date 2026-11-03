//! The app's own log: errors, and a few things worth knowing when something goes wrong (the start, a sync that
//! recovers, a panic). Each entry is a line of JSON in `ledger.log`, in the app's log folder on this machine:
//! never in the vault, so it is never synced. The log takes at most its limit on disk (1 MB unless changed in
//! Settings): when `ledger.log` passes half of it, it moves aside to `ledger.old.log`, replacing the one there, and
//! a new one starts.
//!
//! A debug capture records more for a while: from Start to Stop, every entry also goes to `capture.raw.log`,
//! along with `debug` entries the log itself never keeps (each action the window asks for, each page it opens,
//! each message it shows). Stopping turns it into a `ledger-capture-<time>.log` to send to the developer. The
//! capture is on while that raw file exists, so one that a crash cut short carries on when the app starts again.
//!
//! The log is global, so anything (the syncer's thread, a panic hook) can write to it without a handle to the app.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::ids;

pub const FILE_NAME: &str = "ledger.log";
pub const OLD_FILE_NAME: &str = "ledger.old.log";
pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;
/// The range the limit can be set in.
pub const MIN_MAX_BYTES: u64 = 256 * 1024;
pub const MAX_MAX_BYTES: u64 = 50 * 1024 * 1024;
pub const CAPTURE_RAW: &str = "capture.raw.log";
/// A capture stops growing here, so one left running doesn't fill the disk.
const MAX_CAPTURE_BYTES: u64 = 5 * 1024 * 1024;
/// A message is cut off here, so one huge error can't fill the log.
const MAX_MESSAGE: usize = 8 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub at: String,
    /// `error`, `warn` or `info`; `debug` only in a capture.
    pub level: String,
    /// What it came from: `sync`, `trello`, `app`, `panic`, or the command or screen that failed.
    pub source: String,
    pub message: String,
}

type Listener = Box<dyn Fn(&Entry) + Send + Sync>;

/// A log file, with what to tell when an entry is written.
pub struct Log {
    file: PathBuf,
    lock: Mutex<()>,
    /// The most both files together may take, in bytes.
    max_bytes: AtomicU64,
    listener: Option<Listener>,
}

impl Log {
    pub fn new(dir: &Path, max_bytes: u64, listener: Option<Listener>) -> Self {
        let _ = fs::create_dir_all(dir);
        Log { file: dir.join(FILE_NAME), lock: Mutex::new(()), max_bytes: AtomicU64::new(clamp(max_bytes)), listener }
    }

    pub fn max_bytes(&self) -> u64 {
        self.max_bytes.load(Ordering::Relaxed)
    }

    /// Changes the limit (kept within its range) and brings the files within it at once. Returns the new limit.
    pub fn set_max_bytes(&self, max_bytes: u64) -> u64 {
        let max = clamp(max_bytes);
        self.max_bytes.store(max, Ordering::Relaxed);
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.keep_within();
        max
    }

    fn old_file(&self) -> PathBuf {
        self.file.with_file_name(OLD_FILE_NAME)
    }

    /// Each file may take half the limit. A full log moves aside; an old one over its half (after the limit was
    /// lowered) keeps only its newest lines. Called with the lock held.
    fn keep_within(&self) {
        let half = self.max_bytes() / 2;
        if fs::metadata(&self.file).map(|m| m.len() > half).unwrap_or(false) {
            let _ = fs::rename(&self.file, self.old_file());
        }
        let old = self.old_file();
        if let Ok(text) = fs::read_to_string(&old) {
            if text.len() as u64 > half {
                let mut start = text.len() - half as usize;
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                let tail = &text[start..];
                // Start at a whole line.
                let tail = tail.find('\n').map(|i| &tail[i + 1..]).unwrap_or("");
                let _ = fs::write(&old, tail);
            }
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    fn capture_file(&self) -> PathBuf {
        self.file.with_file_name(CAPTURE_RAW)
    }

    /// Whether a debug capture is running.
    pub fn capturing(&self) -> bool {
        self.capture_file().exists()
    }

    /// Starts a debug capture; one already running carries on.
    pub fn start_capture(&self) -> Result<()> {
        if !self.capturing() {
            let file = self.capture_file();
            fs::write(&file, "").map_err(|e| Error::io(&file, e))?;
            self.write("info", "capture", "Debug capture started");
        }
        Ok(())
    }

    /// Ends the capture and hands back what it recorded, oldest first.
    pub fn stop_capture(&self) -> Result<Vec<Entry>> {
        if !self.capturing() {
            return Err(Error::invalid("No debug capture is running"));
        }
        self.write("info", "capture", "Debug capture stopped");
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let file = self.capture_file();
        let entries = read_lines(&file);
        fs::remove_file(&file).map_err(|e| Error::io(&file, e))?;
        Ok(entries)
    }

    /// The running capture's entries so far (for its start and its count).
    pub fn capture_so_far(&self) -> Vec<Entry> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        read_lines(&self.capture_file())
    }

    /// Keeps a finished capture, already made fit to share, next to the log. Returns where it is.
    pub fn save_capture(&self, name: &str, text: &str) -> Result<PathBuf> {
        let file = self.file.with_file_name(name);
        fs::write(&file, text).map_err(|e| Error::io(&file, e))?;
        Ok(file)
    }

    pub fn write(&self, level: &str, source: &str, message: &str) {
        let mut message = message.trim().to_string();
        if message.len() > MAX_MESSAGE {
            let mut end = MAX_MESSAGE;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
            message.push_str(" …");
        }
        let level = match level {
            "error" | "warn" | "info" | "debug" => level,
            _ => "info",
        };
        let entry = Entry { at: ids::now(), level: level.into(), source: source.trim().into(), message };
        let Ok(line) = serde_json::to_string(&entry) else { return };
        {
            let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
            let capture = self.capture_file();
            let capturing = capture.exists();
            // Debug entries are only for a capture: without one they are dropped.
            if level == "debug" && !capturing {
                return;
            }
            if level != "debug" {
                self.keep_within();
                append(&self.file, &line);
            }
            if capturing && fs::metadata(&capture).map(|m| m.len() < MAX_CAPTURE_BYTES).unwrap_or(false) {
                append(&capture, &line);
            }
        }
        if let Some(listener) = &self.listener {
            listener(&entry);
        }
    }

    /// The last `limit` entries, oldest first. Lines that can't be read (a crash halfway through one) are skipped.
    pub fn read(&self, limit: usize) -> Vec<Entry> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = read_lines(&self.old_file());
        entries.extend(read_lines(&self.file));
        let skip = entries.len().saturating_sub(limit);
        entries.split_off(skip)
    }

    pub fn clear(&self) -> Result<()> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        for file in [self.file.with_file_name(OLD_FILE_NAME), self.file.clone()] {
            match fs::remove_file(&file) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io(&file, e)),
            }
        }
        Ok(())
    }
}

fn append(file: &Path, line: &str) {
    if let Ok(mut out) = OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(out, "{line}");
    }
}

fn read_lines(file: &Path) -> Vec<Entry> {
    fs::read_to_string(file).map(|text| text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()).unwrap_or_default()
}

fn clamp(max_bytes: u64) -> u64 {
    max_bytes.clamp(MIN_MAX_BYTES, MAX_MAX_BYTES)
}

static LOG: OnceLock<Log> = OnceLock::new();

/// Starts the log in `dir`, within `max_bytes`, and has panics written to it too. Called once, when the app starts.
pub fn init(dir: &Path, max_bytes: u64, listener: Listener) {
    if LOG.set(Log::new(dir, max_bytes, Some(listener))).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let place = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let what = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "a panic".into());
        write("error", "panic", &format!("{what}{place}"));
        previous(info);
    }));
}

/// Writes an entry, if the log has been started (in tests it usually hasn't, and nothing is written).
pub fn write(level: &str, source: &str, message: &str) {
    if let Some(log) = LOG.get() {
        log.write(level, source, message);
    }
}

pub fn error(source: &str, message: impl std::fmt::Display) {
    write("error", source, &message.to_string());
}

pub fn info(source: &str, message: impl std::fmt::Display) {
    write("info", source, &message.to_string());
}

/// Only kept while a debug capture runs.
pub fn debug(source: &str, message: impl std::fmt::Display) {
    if LOG.get().is_some_and(|l| l.capturing()) {
        write("debug", source, &message.to_string());
    }
}

fn log() -> Result<&'static Log> {
    LOG.get().ok_or_else(|| Error::invalid("The log hasn't started"))
}

// ---- commands -----------------------------------------------------------------------------------

/// What the About screen shows: this build, this machine and where things are kept.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AboutInfo {
    pub version: String,
    pub tauri_version: String,
    pub os: String,
    pub arch: String,
    pub log_file: String,
    /// The most the log may take on disk, in bytes.
    pub log_max_bytes: u64,
    pub config_dir: String,
    /// The home folder, so a report can put `~` in its place.
    pub home_dir: String,
}

#[tauri::command]
pub fn about_info(app: tauri::AppHandle) -> AboutInfo {
    use tauri::Manager;
    let path = |p: tauri::Result<PathBuf>| p.map(|p| p.display().to_string()).unwrap_or_default();
    AboutInfo {
        version: env!("CARGO_PKG_VERSION").into(),
        tauri_version: tauri::VERSION.into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        log_file: LOG.get().map(|l| l.file().display().to_string()).unwrap_or_default(),
        log_max_bytes: LOG.get().map(|l| l.max_bytes()).unwrap_or(DEFAULT_MAX_BYTES),
        config_dir: path(app.path().app_config_dir()),
        home_dir: path(app.path().home_dir()),
    }
}

#[tauri::command]
pub fn log_write(level: String, source: String, message: String) {
    write(&level, &source, &message);
}

#[tauri::command]
pub fn log_read(limit: Option<usize>) -> Result<Vec<Entry>> {
    Ok(log()?.read(limit.unwrap_or(500)))
}

#[tauri::command]
pub fn log_clear() -> Result<()> {
    log()?.clear()
}

/// Sets the most the log may take on disk, keeps it in this machine's app config, and returns it as set.
#[tauri::command]
pub fn log_set_max_bytes(app: tauri::State<crate::state::App>, bytes: u64) -> Result<u64> {
    let max = log()?.set_max_bytes(bytes);
    let config_dir = app.lock().config_dir.clone();
    crate::settings::update_config(&config_dir, |c| c.log_max_bytes = Some(max))?;
    Ok(max)
}

/// Whether a debug capture runs, since when, and how much it has recorded.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureStatus {
    pub active: bool,
    pub started_at: Option<String>,
    pub events: usize,
}

#[tauri::command]
pub fn capture_status() -> Result<CaptureStatus> {
    let log = log()?;
    if !log.capturing() {
        return Ok(CaptureStatus { active: false, started_at: None, events: 0 });
    }
    let entries = log.capture_so_far();
    Ok(CaptureStatus { active: true, started_at: entries.first().map(|e| e.at.clone()), events: entries.len() })
}

#[tauri::command]
pub fn capture_start() -> Result<CaptureStatus> {
    log()?.start_capture()?;
    capture_status()
}

#[tauri::command]
pub fn capture_debug(source: String, message: String) {
    debug(&source, message);
}

#[tauri::command]
pub fn capture_stop() -> Result<Vec<Entry>> {
    log()?.stop_capture()
}

/// Saves the finished capture (the window has taken out what shouldn't be shared) as `ledger-capture-<time>.log`.
#[tauri::command]
pub fn capture_save(text: String) -> Result<String> {
    let name = format!("ledger-capture-{}.log", chrono::Local::now().format("%Y-%m-%d-%H%M%S"));
    Ok(log()?.save_capture(&name, &text)?.display().to_string())
}
