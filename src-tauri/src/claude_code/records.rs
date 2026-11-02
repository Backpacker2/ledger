//! The record of a run, kept in the thread as `runs/<id>.md`: how the run ended, what Claude Code said at the end,
//! and a short list of what it did. It is written when the run is over and is an ordinary note of the vault: it is
//! synced, encrypted and read like the rest. The raw output, the command line and the text that was sent are not
//! kept: they are for watching a run while it goes, and the prompt itself is already in the thread. Everything is
//! passed through the `Redactor` first, and paths under the project's folder are written relative to it, so the
//! note says nothing about where the vault sits on this machine.

use rusqlite::{Connection, params};
use serde::Serialize;

use super::prompt::RunRequest;
use super::redact::Redactor;
use super::render::{ResultInfo, Step};
use crate::db::query_all;
use crate::error::Result;
use crate::markdown;

/// How much one note holds: the steps it lists, the length of the closing message, the deliverables it names.
const MAX_STEPS: usize = 200;
const MAX_ANSWER_CHARS: usize = 20_000;
const MAX_FILES: usize = 20;

/// What is kept of a run. `status` is "finished", "failed" or "cancelled".
#[derive(Debug, Clone, PartialEq)]
pub struct RunRecord {
    pub id: String,
    pub project: String,
    pub thread: String,
    pub prompt_id: String,
    pub status: String,
    pub model: String,
    pub permission_mode: String,
    pub started_at: String,
    pub finished_at: String,
    pub seconds: u64,
    pub turns: Option<u64>,
    /// The deliverables the run produced, relative to the project's `deliverables/` folder: the versions that were
    /// linked to the prompt, or when none were, the files it changed.
    pub files: Vec<String>,
    pub body: String,
}

/// A record as the prompt's card shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecordView {
    pub id: String,
    pub prompt_id: String,
    pub status: String,
    pub model: String,
    pub permission_mode: String,
    pub started_at: String,
    pub finished_at: String,
    pub seconds: i64,
    pub turns: Option<i64>,
    pub files: Vec<String>,
    pub html: String,
}

/// What the run itself knows once it is over.
pub struct RunFacts<'a> {
    pub id: &'a str,
    pub status: &'a str,
    /// Why it failed or stopped, in a sentence.
    pub message: Option<&'a str>,
    pub model: &'a str,
    pub permission_mode: &'a str,
    pub started_at: &'a str,
    pub finished_at: &'a str,
    pub seconds: u64,
    pub result: Option<&'a ResultInfo>,
    pub steps: &'a [Step],
    /// The deliverables the run produced: the versions linked to the prompt, or the files it changed.
    pub files: &'a [String],
}

pub fn build(request: &RunRequest, facts: &RunFacts, redactor: &Redactor) -> RunRecord {
    let cwd = request.cwd.display().to_string();
    RunRecord {
        id: facts.id.to_string(),
        project: request.project.clone(),
        thread: request.thread.clone(),
        prompt_id: request.prompt_id.clone(),
        status: facts.status.to_string(),
        model: redactor.apply(facts.model),
        permission_mode: facts.permission_mode.to_string(),
        started_at: facts.started_at.to_string(),
        finished_at: facts.finished_at.to_string(),
        seconds: facts.seconds,
        turns: facts.result.and_then(|r| r.turns),
        files: facts.files.iter().take(MAX_FILES).cloned().collect(),
        body: redactor.apply(&body(&cwd, facts)),
    }
}

/// How it ended, then what it did.
fn body(cwd: &str, facts: &RunFacts) -> String {
    let answer = facts.result.filter(|r| !r.is_error).map(|r| r.text.trim()).filter(|t| !t.is_empty());
    let mut out = match facts.status {
        "finished" => match answer {
            Some(text) => without_folder(&shorten(text, MAX_ANSWER_CHARS), cwd),
            None => "Claude Code finished without a closing message.".to_string(),
        },
        "cancelled" => facts.message.unwrap_or("Stopped at your request").to_string(),
        _ => format!("Failed: {}", facts.message.unwrap_or("Claude Code reported an error")),
    };
    out.push('\n');

    let denials = facts.result.map_or(&[][..], |r| r.denials.as_slice());
    if !facts.steps.is_empty() || !denials.is_empty() {
        out.push_str("\n## Activity\n\n");
        for step in facts.steps.iter().take(MAX_STEPS) {
            out.push_str(&step_line(step, cwd));
            out.push('\n');
        }
        if facts.steps.len() > MAX_STEPS {
            out.push_str(&format!("- …and {} more\n", facts.steps.len() - MAX_STEPS));
        }
        for denied in denials.iter().take(10) {
            out.push_str(&format!("- Refused by the permission mode: `{}`\n", without_folder(denied, cwd).replace('`', "'")));
        }
    }
    out
}

fn step_line(step: &Step, cwd: &str) -> String {
    if step.failed {
        return format!("- Failed: {}", without_folder(&step.detail, cwd));
    }
    let detail = relative(&step.detail, cwd);
    if detail.is_empty() { format!("- {}", step.name) } else { format!("- {} `{}`", step.name, detail.replace('`', "'")) }
}

// ---- paths and text ---------------------------------------------------------------------------------

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    let same = if cfg!(windows) { head.eq_ignore_ascii_case(prefix) } else { head == prefix };
    same.then(|| &text[prefix.len()..])
}

/// A path under the project's folder as the note names it: relative, with `/`. Any other text comes back as it was.
fn relative(path: &str, cwd: &str) -> String {
    let folder = cwd.trim_end_matches(['/', '\\']);
    if !folder.is_empty()
        && let Some(rest) = strip_prefix_ci(path, folder).filter(|rest| rest.starts_with(['/', '\\']))
    {
        return rest.trim_start_matches(['/', '\\']).replace('\\', "/");
    }
    path.to_string()
}

/// Free text with the project's folder taken out of any path in it.
fn without_folder(text: &str, cwd: &str) -> String {
    let folder = cwd.trim_end_matches(['/', '\\']);
    if folder.is_empty() {
        return text.to_string();
    }
    let slashed = folder.replace('\\', "/");
    text.replace(&format!("{folder}\\"), "").replace(&format!("{folder}/"), "").replace(&format!("{slashed}/"), "")
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

// ---- the database -----------------------------------------------------------------------------------

/// Adds a record. Fails if its thread is gone (a record belongs to one).
pub fn insert(conn: &Connection, record: &RunRecord) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO claude_runs (id, project_slug, thread_slug, prompt_id, status, model, permission_mode, started_at, finished_at, seconds, turns, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            record.id,
            record.project,
            record.thread,
            record.prompt_id,
            record.status,
            record.model,
            record.permission_mode,
            record.started_at,
            record.finished_at,
            record.seconds as i64,
            record.turns.map(|t| t as i64),
            record.body
        ],
    )?;
    for (position, path) in record.files.iter().enumerate() {
        tx.execute("INSERT OR IGNORE INTO claude_run_files (run_id, position, path) VALUES (?1, ?2, ?3)", params![record.id, position as i64, path])?;
    }
    tx.commit()?;
    Ok(())
}

/// The runs of one prompt, newest first.
pub fn for_prompt(conn: &Connection, project: &str, thread: &str, prompt_id: &str) -> Result<Vec<RunRecordView>> {
    let mut views = query_all(
        conn,
        "SELECT id, prompt_id, status, model, permission_mode, started_at, finished_at, seconds, turns, body
         FROM claude_runs WHERE project_slug = ?1 AND thread_slug = ?2 AND prompt_id = ?3 ORDER BY started_at DESC, rowid DESC",
        [project, thread, prompt_id],
        |r| {
            let body: String = r.get(9)?;
            Ok(RunRecordView {
                id: r.get(0)?,
                prompt_id: r.get(1)?,
                status: r.get(2)?,
                model: r.get(3)?,
                permission_mode: r.get(4)?,
                started_at: r.get(5)?,
                finished_at: r.get(6)?,
                seconds: r.get(7)?,
                turns: r.get(8)?,
                files: vec![],
                html: markdown::render(&body),
            })
        },
    )?;
    for view in &mut views {
        view.files = query_all(conn, "SELECT path FROM claude_run_files WHERE run_id = ?1 ORDER BY position", [&view.id], |r| r.get(0))?;
    }
    Ok(views)
}
