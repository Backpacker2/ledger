//! What you did, and when: the events behind the contribution graph on the dashboard.
//!
//! Every thing you make counts once, on the moment it was made: a thread, a post you wrote, a prompt, feedback, an
//! upload, a published version, a run of a prompt with Claude Code, a user story, a task. What the app writes for
//! you (a template's guiding questions, the "version published" note) doesn't count, and neither do edits, which
//! leave no history. The frontend groups the events into days, in local time.

use rusqlite::Connection;

use crate::db::query_all;
use crate::error::Result;
use crate::models::ActivityEvent;

const EVENTS: &str = "
    SELECT project_slug, 'thread', created_at FROM threads
    UNION ALL SELECT project_slug, 'post', created_at FROM posts WHERE guiding_question = 0 AND version_published = 0
    UNION ALL SELECT project_slug, 'prompt', created_at FROM prompts
    UNION ALL SELECT project_slug, 'feedback', created_at FROM feedback
    UNION ALL SELECT project_slug, 'upload', uploaded_at FROM uploads
    UNION ALL SELECT project_slug, 'version', created_at FROM version_records
    UNION ALL SELECT project_slug, 'run', started_at FROM claude_runs
    UNION ALL SELECT project_slug, 'story', created_at FROM user_stories
    UNION ALL SELECT project_slug, 'task', created_at FROM tasks
    ORDER BY 3";

/// Every event in the vault, oldest first.
pub fn list(conn: &Connection) -> Result<Vec<ActivityEvent>> {
    query_all(conn, EVENTS, [], |r| Ok(ActivityEvent { project_slug: r.get(0)?, kind: r.get(1)?, at: r.get(2)? }))
}
