//! After a run: what it wrote is read into the vault the way an edit made in Obsidian would be (the same check
//! the app makes when its window regains focus), and the deliverables it wrote are published and linked to the prompt
//! (see `versions` for the rules), so the prompt shows what came out of it.

use super::prompt::RunRequest;
use super::records::{self, RunRecord};
use super::runs::VaultOutcome;
use super::versions::Change;
use crate::error::Result;
use crate::state::Core;
use crate::{deliverables, prompts};

/// Keeps the note about a run in its thread, and writes it to the vault's files. Fails if the thread is gone.
pub fn keep_record(core: &mut Core, record: &RunRecord) -> Result<()> {
    records::insert(&core.conn, record)?;
    core.vault.persist(&core.conn)?;
    Ok(())
}

/// More than this many changed files in one run is a sign that something other than deliverables was written.
const MAX_CHANGES: usize = 20;

/// Reloads the vault if its files changed behind the app's back. Then, for each deliverable the run changed:
/// edits that no version held are published first, whatever became of the run (they would otherwise be lost under
/// the run's changes), and, when `link` is set (the run finished properly), the run's result is published as the next
/// version and that version is linked to the prompt.
pub fn refresh_vault(core: &mut Core, request: &RunRequest, changes: &[Change], link: bool) -> VaultOutcome {
    let mut outcome = VaultOutcome::default();
    if core.vault.changed_on_disk() {
        match core.reload() {
            Ok(()) => outcome.reloaded = true,
            Err(e) => {
                outcome.error = Some(e.to_string());
                return outcome;
            }
        }
    }

    let root = core.vault.root().to_path_buf();
    let mut published = false;
    for change in changes.iter().take(MAX_CHANGES) {
        let base = change.path();
        if let Some(held) = change.held() {
            match deliverables::publish_run(&core.conn, &root, &request.project, base, Some(held), &request.thread) {
                Ok(version) => {
                    outcome.kept.push(deliverables::snapshot_path(base, &version));
                    published = true;
                }
                Err(e) => {
                    // The run's result is not published over edits that weren't kept.
                    outcome.error = Some(format!("The earlier edits to {base} couldn't be kept as a version: {e}"));
                    continue;
                }
            }
        }
        if !link {
            continue;
        }
        let version = match deliverables::publish_run(&core.conn, &root, &request.project, base, None, &request.thread) {
            Ok(version) => version,
            Err(e) => {
                outcome.error = Some(format!("{base} couldn't be published: {e}"));
                continue;
            }
        };
        published = true;
        let path = deliverables::snapshot_path(base, &version);
        match prompts::link_deliverable(&core.conn, &request.project, &request.thread, &request.prompt_id, &path) {
            Ok(_) => outcome.linked.push(path),
            Err(e) => outcome.error = Some(format!("{path} was published but couldn't be linked to the prompt: {e}")),
        }
    }
    if published && let Err(e) = core.vault.persist(&core.conn) {
        outcome.error = Some(e.to_string());
    }
    outcome
}
