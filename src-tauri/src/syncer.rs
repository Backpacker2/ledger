//! The background worker that keeps the vault and GitHub together: it syncs shortly after the vault changes
//! (waiting for a pause in the typing), on request, and every few minutes to notice what other machines pushed.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::Result;
use crate::github::GithubRemote;
use crate::ids;
use crate::settings::SyncConfig;
use crate::state::Core;
use crate::sync::{self, Applier, LocalOp};

/// How long the vault must stay quiet before changes are pushed, and the longest a steady stream of edits can
/// postpone it.
const DEBOUNCE: Duration = Duration::from_secs(5);
const MAX_DEBOUNCE: Duration = Duration::from_secs(60);
/// How often to look for changes made elsewhere, and how soon to try again after a failure.
const POLL: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub configured: bool,
    pub repo: Option<String>,
    pub branch: Option<String>,
    /// "disabled", "idle", "syncing" or "error".
    pub state: String,
    /// Changes made here that have not been pushed yet.
    pub pending: bool,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    pub summary: Option<String>,
}

pub enum Event {
    Status(SyncStatus),
    /// Files were changed by a pull and the in-memory database reloaded: screens should refresh.
    VaultChanged,
}

pub type Emit = Arc<dyn Fn(Event) + Send + Sync>;

enum Msg {
    Dirty,
    Now,
}

pub struct Syncer {
    tx: Mutex<Sender<Msg>>,
    status: Arc<Mutex<SyncStatus>>,
    emit: Emit,
}

impl Syncer {
    /// Something in the vault changed.
    pub fn dirty(&self) {
        let _ = self.tx.lock().unwrap_or_else(PoisonError::into_inner).send(Msg::Dirty);
    }

    /// Sync right away.
    pub fn now(&self) {
        let _ = self.tx.lock().unwrap_or_else(PoisonError::into_inner).send(Msg::Now);
    }

    pub fn status(&self) -> SyncStatus {
        lock(&self.status).clone()
    }

    /// Applies new settings: records whether sync is on and, if it is, starts with a sync.
    pub fn reconfigure(&self, config: Option<&SyncConfig>) {
        update(&self.status, &self.emit, |s| {
            s.configured = config.is_some();
            s.repo = config.map(|c| c.repo.clone());
            s.branch = config.map(|c| c.branch.clone());
            s.state = if config.is_some() { "idle" } else { "disabled" }.to_string();
            s.pending = false;
            s.last_error = None;
            if config.is_none() {
                s.summary = None;
                s.last_sync_at = None;
            }
        });
        if config.is_some() {
            self.now();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn update(status: &Mutex<SyncStatus>, emit: &Emit, change: impl FnOnce(&mut SyncStatus)) {
    let snapshot = {
        let mut guard = lock(status);
        change(&mut guard);
        guard.clone()
    };
    emit(Event::Status(snapshot));
}

pub fn spawn(core: Arc<Mutex<Core>>, emit: Emit) -> Syncer {
    let (tx, rx) = channel();
    let status = Arc::new(Mutex::new(SyncStatus { state: "disabled".into(), ..SyncStatus::default() }));
    {
        let (status, emit) = (status.clone(), emit.clone());
        std::thread::spawn(move || worker(rx, core, status, emit));
    }
    Syncer { tx: Mutex::new(tx), status, emit }
}

fn worker(rx: Receiver<Msg>, core: Arc<Mutex<Core>>, status: Arc<Mutex<SyncStatus>>, emit: Emit) {
    let mut pending_since: Option<Instant> = None;
    let mut failed = false;

    loop {
        let wait = if pending_since.is_some() {
            DEBOUNCE
        } else if failed {
            RETRY
        } else {
            POLL
        };

        let run = match rx.recv_timeout(wait) {
            Ok(Msg::Dirty) => {
                let configured = lock(&core).sync.is_some();
                if configured {
                    let since = *pending_since.get_or_insert_with(Instant::now);
                    update(&status, &emit, |s| s.pending = true);
                    since.elapsed() >= MAX_DEBOUNCE
                } else {
                    false
                }
            }
            Ok(Msg::Now) | Err(RecvTimeoutError::Timeout) => true,
            Err(RecvTimeoutError::Disconnected) => return,
        };

        if run {
            pending_since = None;
            failed = !run_once(&core, &status, &emit);
        }
    }
}

/// One sync. Returns whether it worked.
fn run_once(core: &Arc<Mutex<Core>>, status: &Mutex<SyncStatus>, emit: &Emit) -> bool {
    let (root, config) = {
        let core = lock(core);
        (core.vault.root().to_path_buf(), core.sync.clone())
    };
    let Some(config) = config else {
        update(status, emit, |s| {
            s.configured = false;
            s.state = "disabled".into();
            s.pending = false;
        });
        return true;
    };

    update(status, emit, |s| {
        s.configured = true;
        s.repo = Some(config.repo.clone());
        s.branch = Some(config.branch.clone());
        s.state = "syncing".into();
    });

    let result = GithubRemote::new(&config).and_then(|remote| {
        remote.ensure_private()?;
        sync::run(&root, &config.repo, &config.branch, &remote, &CoreApplier { core: core.clone() })
    });

    match result {
        Ok(outcome) => {
            update(status, emit, |s| {
                s.state = "idle".into();
                s.pending = false;
                s.last_error = None;
                s.last_sync_at = Some(ids::now());
                s.summary = Some(outcome.summary());
            });
            if outcome.changed_local() {
                emit(Event::VaultChanged);
            }
            true
        }
        Err(e) => {
            update(status, emit, |s| {
                s.state = "error".into();
                s.last_error = Some(e.to_string());
            });
            false
        }
    }
}

/// Lets a pull write into the vault while holding the vault lock, and reloads the in-memory database after.
struct CoreApplier {
    core: Arc<Mutex<Core>>,
}

impl Applier for CoreApplier {
    fn apply(&self, ops: Vec<LocalOp>) -> Result<Vec<String>> {
        let mut core = lock(&self.core);
        let root = core.vault.root().to_path_buf();
        let (skipped, wrote) = sync::apply_to_disk(&root, ops)?;
        if wrote {
            core.reload()?;
        }
        Ok(skipped)
    }
}
