//! The shapes that cross the command boundary. Responses are flat camelCase JSON the frontend consumes
//! directly: the stored records plus rendered HTML where markdown is involved.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedThread {
    pub slug: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub slug: String,
    pub title: String,
    pub created_at: String,
    pub linked_thread_slugs: Vec<String>,
    pub post_count: i64,
    pub linked_count: i64,
    pub last_activity: String,
    pub document_count: i64,
    pub story_count: i64,
}

/// `guiding_question`: seeded from a document type's guiding questions.
/// `version_published`: an automatic note announcing a published version; `document_id` and
/// `document_version` point at the snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PostView {
    pub id: String,
    pub parent_id: Option<String>,
    pub guiding_question: bool,
    pub version_published: bool,
    pub document_id: Option<String>,
    pub document_version: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub body: String,
    pub html: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadView {
    pub slug: String,
    pub title: String,
    pub created_at: String,
    pub linked_threads: Vec<LinkedThread>,
    pub posts: Vec<PostView>,
    pub documents: Vec<DocumentSummary>,
    pub stories: Vec<StoryView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentType {
    pub slug: String,
    pub name: String,
    /// The "use when…" hint.
    pub trigger: String,
    pub questions: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub document_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String,
    pub document_type_slug: String,
    pub document_type_name: String,
    pub title: String,
    pub thread_slug: Option<String>,
    pub thread_title: Option<String>,
    pub version: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// A live document, or (`snapshot`) one of its published versions. A snapshot whose draft has since been
/// deleted is `deleted`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentView {
    pub id: String,
    pub document_type_slug: String,
    pub document_type_name: String,
    pub title: String,
    pub thread_slug: Option<String>,
    pub thread_title: Option<String>,
    pub version: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub body: String,
    pub html: String,
    /// Published versions, oldest first.
    pub versions: Vec<String>,
    pub snapshot: bool,
    pub deleted: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Criterion {
    pub text: String,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoryView {
    pub id: String,
    pub summary: String,
    /// "As a …"
    pub role: String,
    /// "I want to …"
    pub want: String,
    /// "Such that …"
    pub reason: String,
    pub criteria: Vec<Criterion>,
    pub column_id: String,
    pub threads: Vec<LinkedThread>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Planboard {
    pub columns: Vec<Column>,
    /// Where a newly created user story lands.
    pub backlog_column_id: String,
    pub user_stories: Vec<StoryView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// "post" or "document".
    pub kind: String,
    /// The post or document id.
    pub id: String,
    /// The thread's title for a post, the document's title for a document.
    pub title: String,
    pub thread_slug: Option<String>,
    pub thread_title: Option<String>,
    pub created_at: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub version: String,
    pub db_path: String,
}

// ---- requests -----------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Bump {
    #[default]
    Minor,
    Major,
}

/// What the caller supplies when creating a story.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoryDraft {
    pub id: String,
    pub summary: String,
    pub role: String,
    pub want: String,
    pub reason: String,
    pub criteria: Vec<Criterion>,
    pub thread_slugs: Vec<String>,
}

/// Only the fields that are present change; `criteria` replaces the whole list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoryUpdate {
    pub summary: Option<String>,
    pub role: Option<String>,
    pub want: Option<String>,
    pub reason: Option<String>,
    pub criteria: Option<Vec<Criterion>>,
    pub column_id: Option<String>,
}
