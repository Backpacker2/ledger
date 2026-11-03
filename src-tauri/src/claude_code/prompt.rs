//! What gets sent to Claude Code for a prompt, and where it runs.
//!
//! The prompt is run in its project's folder (`<vault>/projects/<project>/`): there it can read the project's
//! README (the reading guide), its threads and its other deliverables, and write the deliverable where this app
//! looks for it, in `deliverables/`. Around the prompt's own text goes a short, fixed preamble that says exactly
//! that and nothing more. The whole text is shown to the person before anything is started.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::db::query_opt;
use crate::error::{Error, Result};
use crate::models::{PromptDeliverable, PromptView, TemplateView};
use crate::{deliverables, projects, prompts, templates, threads, uploads};

/// A prompt may be long, but this long means something went wrong.
pub const MAX_PROMPT_BYTES: usize = 1024 * 1024;
const MAX_LISTED_FILES: usize = 5000;

/// Everything a run needs to know about the prompt it runs.
#[derive(Debug, Clone)]
pub struct RunRequest {
    pub project: String,
    pub thread: String,
    pub prompt_id: String,
    pub prompt_title: String,
    /// The run this request is for, once it has started; empty before.
    pub run_id: String,
    /// The project's folder: where the program runs.
    pub cwd: PathBuf,
    /// The text sent on standard input.
    pub text: String,
}

/// A name from the frontend that is used as a folder name must be one plain name.
fn plain_segment(segment: &str) -> bool {
    !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(['/', '\\', ':', '\0'])
}

/// Checks that the prompt is there, in a project that is there, and puts together what to send. Nothing is started
/// and (apart from making sure `deliverables/` exists, so the instruction to use it is true) nothing is written.
pub fn prepare(conn: &Connection, root: &Path, project: &str, thread: &str, prompt_id: &str) -> Result<RunRequest> {
    if !plain_segment(project) || !plain_segment(thread) || !plain_segment(prompt_id) {
        return Err(Error::invalid("That isn't a prompt in this vault"));
    }
    projects::require(conn, project)?;
    threads::require(conn, project, thread)?;
    let prompt = prompts::get(conn, project, thread, prompt_id)?;
    let name = query_opt(conn, "SELECT name FROM projects WHERE slug = ?1", [project], |r| r.get::<_, String>(0))?.unwrap_or_default();

    let cwd = uploads::project_dir(root, project);
    if !cwd.is_dir() {
        return Err(Error::invalid(format!(
            "The project's folder isn't on disk ({}). Use Reload from disk under Settings, or check that the vault is where you expect it.",
            cwd.display()
        )));
    }
    let dir = deliverables::deliverables_dir(root, project);
    fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;

    let template = prompt.template_id.as_deref().and_then(|id| templates::find(conn, project, id).ok());
    let text = compose(&name, &prompt, template.as_ref());
    if text.len() > MAX_PROMPT_BYTES {
        return Err(Error::invalid("That prompt is too long to send"));
    }
    Ok(RunRequest {
        project: project.to_string(),
        thread: thread.to_string(),
        prompt_id: prompt.id.clone(),
        prompt_title: prompt.title.clone(),
        run_id: String::new(),
        cwd,
        text,
    })
}

/// What is sent must be what the person saw: a prompt edited since the confirmation was shown is not run.
pub fn ensure_confirmed(request: &RunRequest, confirmed: &str) -> Result<()> {
    if request.text == confirmed {
        Ok(())
    } else {
        Err(Error::invalid("The prompt changed after you looked at it. Open the confirmation again to see what will be sent."))
    }
}

/// One line of text: names are the user's, and a stray newline in one must not read as an instruction.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A code fence longer than any run of backticks inside `text`, so quoting a template can't end early.
fn fence_for(text: &str) -> String {
    let (mut longest, mut run) = (0, 0);
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat((longest + 1).max(3))
}

/// The text sent to Claude Code: a preamble, a line, and the prompt exactly as written.
pub fn compose(project_name: &str, prompt: &PromptView, template: Option<&TemplateView>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "You are working in the folder of the Ledger project \"{}\": your working directory. It is a vault of markdown files, and README.md is the project's reading guide, so read that first.\n\n",
        one_line(project_name)
    ));
    out.push_str(
        "Put what the request below asks for in the deliverables/ folder, as a markdown file unless the request says otherwise. Stay inside this folder, and leave published versions (names like plan@v1.0.md) alone.\n",
    );

    let linked: &[PromptDeliverable] = &prompt.deliverables;
    if !linked.is_empty() {
        let names: Vec<String> =
            linked.iter().map(|d| format!("deliverables/{}{}", d.path, if d.exists { "" } else { " (not written yet)" })).collect();
        out.push_str(&format!("This request is already linked to {}. Write into those rather than creating other files.\n", names.join(", ")));
    }
    out.push_str(&format!("The discussion the request came out of is in threads/{}/ (thread.md, and the posts/ folder next to it).\n", prompt.thread_slug));

    // A deliverable that exists already has the template's structure; one still to be written gets it here.
    if let Some(template) = template.filter(|t| !t.body.trim().is_empty())
        && !linked.iter().any(|d| d.exists)
    {
        let body = template.body.trim_end();
        let fence = fence_for(body);
        out.push_str(&format!("\nUse this structure for the deliverable (the template \"{}\"):\n\n{fence}markdown\n{body}\n{fence}\n", one_line(&template.name)));
    }

    out.push_str("\nWhen you are done, say in a sentence or two which files you wrote.\n\n---\n\n");
    if !prompt.title.trim().is_empty() {
        out.push_str(&format!("# {}\n\n", one_line(&prompt.title)));
    }
    out.push_str(prompt.body.trim());
    out.push('\n');
    out
}

/// The files under a deliverables folder, as paths relative to it with `/` (the way the app names deliverables).
/// Dotfiles are skipped, as the vault skips them, and links are not followed.
pub fn list_files(dir: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            if out.len() >= MAX_LISTED_FILES {
                return;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else { continue };
            if name.starts_with('.') {
                continue;
            }
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path(), &path, out),
                Ok(kind) if kind.is_file() => {
                    out.insert(path);
                }
                _ => {}
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(dir, "", &mut out);
    out
}
