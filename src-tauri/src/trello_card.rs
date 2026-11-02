//! What a Ledger task or user story looks like as a Trello card, and back.
//!
//! * A **task** is a card whose name is the task's title and whose description is the task's body, as it is.
//! * A **user story** is a card whose name is the story's summary and whose description is this, every part left
//!   out when it is empty:
//!
//!   ```text
//!   **As a** customer
//!   **I want to** see prices
//!   **Such that** I can decide
//!
//!   **Acceptance criteria**
//!   - [ ] A page lists plans
//!   - [x] Prices in euro
//!
//!   **Notes**
//!   Raised on the first call.
//!   ```
//!
//!   Reading it back is forgiving: Trello has no idea of this format, so a person may edit the description by hand.
//!   Text that isn't in any recognised part is never dropped, it goes in front of the notes; a description with no
//!   recognised part at all is all notes. The notes come last and are taken verbatim, so nothing in them can be
//!   mistaken for a heading.
//!
//! Every description Ledger writes ends with a short footer (`---` and "Synced with Ledger"), which marks the card as
//! one Ledger made (see [`with_footer`]); it is taken off again before a description is read or compared.
//!
//! Two sides count as the same when their *canonical* forms are equal, so a description that was only formatted
//! differently never looks like an edit (see [`Content`]).

use serde::{Deserialize, Serialize};

use crate::models::{StoryView, TaskView};
use crate::trello_api::Card;

pub const STORY_LABEL: &str = "User story";
pub const TASK_LABEL: &str = "Task";
pub const STORY_COLOR: &str = "purple";
pub const TASK_COLOR: &str = "sky";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Task,
    Story,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Story => "user story",
        }
    }

    pub fn folder(self) -> &'static str {
        match self {
            Kind::Task => "tasks",
            Kind::Story => "userstories",
        }
    }
}

/// The parts of a story that live in the card's description.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoryText {
    pub role: String,
    pub want: String,
    pub reason: String,
    pub criteria: Vec<Check>,
    pub notes: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Check {
    pub text: String,
    pub done: bool,
}

/// What is compared between the two sides: the title, the description in canonical form, and for a task the
/// Trello list it sits in (`None` when its column has no list, which is then left alone).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Content {
    pub title: String,
    pub desc: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<String>,
}

/// Line endings and surrounding blank space are not edits.
pub fn norm(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n").trim().to_string()
}

// ---- stories ------------------------------------------------------------------------------------------

const ROLE: &str = "**As a**";
const WANT: &str = "**I want to**";
const REASON: &str = "**Such that**";
const CRITERIA: &str = "**Acceptance criteria**";
const NOTES: &str = "**Notes**";

pub fn story_text(story: &StoryView) -> StoryText {
    StoryText {
        role: norm(&story.role),
        want: norm(&story.want),
        reason: norm(&story.reason),
        criteria: story.criteria.iter().map(|c| Check { text: norm(&c.text), done: c.done }).collect(),
        notes: norm(&story.notes),
    }
}

pub fn render_story(text: &StoryText) -> String {
    let mut blocks: Vec<String> = vec![];

    let head: Vec<String> = [(ROLE, &text.role), (WANT, &text.want), (REASON, &text.reason)]
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(marker, value)| format!("{marker} {value}"))
        .collect();
    if !head.is_empty() {
        blocks.push(head.join("\n"));
    }
    if !text.criteria.is_empty() {
        let items: Vec<String> = text.criteria.iter().map(|c| format!("- [{}] {}", if c.done { 'x' } else { ' ' }, c.text)).collect();
        blocks.push(format!("{CRITERIA}\n{}", items.join("\n")));
    }
    if !text.notes.is_empty() {
        blocks.push(format!("{NOTES}\n{}", text.notes));
    }
    blocks.join("\n\n")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    /// Before anything recognised, or after a blank line that ended a part: text that belongs nowhere yet.
    Loose,
    Role,
    Want,
    Reason,
    Criteria,
    Notes,
}

fn strip_marker<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let line = line.trim_start();
    let head = line.get(..marker.len())?;
    head.eq_ignore_ascii_case(marker).then(|| line[marker.len()..].trim())
}

/// `- [ ] text`, `* [x] text`, or a bare `- text`.
fn criterion(line: &str) -> Option<Check> {
    let line = line.trim_start();
    let rest = line.strip_prefix(['-', '*', '+'])?.strip_prefix(' ').or_else(|| line[1..].strip_prefix('\t'))?.trim_start();
    for (open, done) in [("[ ]", false), ("[x]", true), ("[X]", true)] {
        if let Some(text) = rest.strip_prefix(open) {
            return Some(Check { text: text.trim().to_string(), done });
        }
    }
    Some(Check { text: rest.trim().to_string(), done: false })
}

pub fn parse_story(desc: &str) -> StoryText {
    parse_story_marked(desc).0
}

/// What the description says, and whether it has the format's parts at all. A description with none of them is a
/// card somebody wrote in Trello: it has nothing to say about the story's role, goal, reason or criteria, so
/// whoever reads it keeps those as they are and takes only its text.
pub fn parse_story_marked(desc: &str) -> (StoryText, bool) {
    let desc = norm(desc);
    let mut text = StoryText::default();
    let mut loose: Vec<&str> = vec![];
    let mut notes: Vec<&str> = vec![];
    let mut part = Part::Loose;
    let mut marked = false;

    let mut fields = [String::new(), String::new(), String::new()];
    let field = |part: Part| match part {
        Part::Role => Some(0),
        Part::Want => Some(1),
        Part::Reason => Some(2),
        _ => None,
    };

    for line in desc.lines() {
        if part == Part::Notes {
            notes.push(line);
            continue;
        }
        if let Some(rest) = strip_marker(line, NOTES) {
            marked = true;
            part = Part::Notes;
            if !rest.is_empty() {
                notes.push(rest);
            }
            continue;
        }
        if let Some(rest) = strip_marker(line, CRITERIA) {
            marked = true;
            part = Part::Criteria;
            if !rest.is_empty() {
                loose.push(rest);
            }
            continue;
        }
        let started = [(ROLE, Part::Role), (WANT, Part::Want), (REASON, Part::Reason)]
            .into_iter()
            .find_map(|(marker, next)| strip_marker(line, marker).map(|rest| (next, rest)));
        if let Some((next, rest)) = started {
            marked = true;
            part = next;
            // A field that is given twice keeps its first value; the second goes to the loose text.
            let i = field(next).unwrap_or(0);
            if fields[i].is_empty() {
                fields[i] = rest.to_string();
            } else {
                loose.push(line);
                part = Part::Loose;
            }
            continue;
        }

        let blank = line.trim().is_empty();
        match part {
            Part::Role | Part::Want | Part::Reason => {
                if blank {
                    part = Part::Loose;
                } else if let Some(i) = field(part) {
                    // A field that goes on over several lines.
                    fields[i] = format!("{}\n{}", fields[i], line.trim());
                }
            }
            Part::Criteria => {
                if blank {
                    // A blank line ends the list.
                    part = Part::Loose;
                } else if let Some(check) = criterion(line) {
                    if !check.text.is_empty() {
                        text.criteria.push(check);
                    }
                } else {
                    loose.push(line);
                }
            }
            Part::Loose => loose.push(line),
            Part::Notes => unreachable!("handled above"),
        }
    }

    let [role, want, reason] = fields;
    text.role = role.trim().to_string();
    text.want = want.trim().to_string();
    text.reason = reason.trim().to_string();

    let loose = norm(&loose.join("\n"));
    let notes = norm(&notes.join("\n"));
    text.notes = match (loose.is_empty(), notes.is_empty()) {
        (true, _) => notes,
        (false, true) => loose,
        (false, false) => format!("{loose}\n\n{notes}"),
    };
    (text, marked)
}

/// A description in the form [`render_story`] makes of what it says: how the two sides are compared.
pub fn canonical_story_desc(desc: &str) -> String {
    render_story(&parse_story(desc))
}

// ---- the two sides as content -------------------------------------------------------------------------

pub fn task_content(task: &TaskView, list: Option<&str>) -> Content {
    Content { title: norm(&task.title), desc: norm(&task.body), list: list.map(str::to_string) }
}

/// The story as it would be pushed, put through the same reading as a card coming back, so that a story that
/// doesn't survive the trip (a value with a blank line in it, say) compares equal to the card it made.
pub fn story_content(story: &StoryView) -> Content {
    Content { title: norm(&story.summary), desc: canonical_story_desc(&render_story(&story_text(story))), list: None }
}

pub fn card_task_content(card: &Card) -> Content {
    Content { title: norm(&card.name), desc: card_text(card), list: Some(card.id_list.clone()) }
}

pub fn card_story_content(card: &Card) -> Content {
    Content { title: norm(&card.name), desc: canonical_story_desc(&card_text(card)), list: None }
}

// ---- the footer ---------------------------------------------------------------------------------------

/// The last lines of the description of every card Ledger writes. They mark the card as made by Ledger: such a card
/// reaches other machines through the vault (the item's `trelloCard`), so a machine that finds one it has no item
/// for doesn't add it, which would make a second item for the same card while the first is still on its way, or
/// bring back one that was deleted. Taking the lines off the description brings the card in again.
const FOOTER: &str = "---\n_Synced with Ledger_";

/// The description as it is written to Trello: the text, then the footer.
pub fn with_footer(desc: &str) -> String {
    if desc.is_empty() { FOOTER.to_string() } else { format!("{desc}\n\n{FOOTER}") }
}

/// The description without the footer, and whether it had one.
pub fn strip_footer(desc: &str) -> (String, bool) {
    let desc = norm(desc);
    match desc.strip_suffix(FOOTER) {
        Some(rest) if rest.is_empty() || rest.ends_with("\n\n") => (norm(rest), true),
        _ => (desc, false),
    }
}

/// The card's description as text for the item: without the footer, in normal form.
pub fn card_text(card: &Card) -> String {
    strip_footer(&card.desc).0
}

pub fn made_by_ledger(card: &Card) -> bool {
    strip_footer(&card.desc).1
}

// ---- labels -------------------------------------------------------------------------------------------

/// The ids of the board's two kind labels.
#[derive(Debug, Clone, Default)]
pub struct KindLabels {
    pub story: String,
    pub task: String,
}

impl KindLabels {
    pub fn id(&self, kind: Kind) -> &str {
        match kind {
            Kind::Task => &self.task,
            Kind::Story => &self.story,
        }
    }

    /// What a card with no item yet becomes: a story when it carries the story label, otherwise a task.
    pub fn new_kind(&self, card: &Card) -> Kind {
        if card.id_labels.contains(&self.story) { Kind::Story } else { Kind::Task }
    }

    /// What a card says its linked item is. A card that carries neither label, or its own item's, leaves the item
    /// as it is; only the other kind's label turns it into the other kind. With both labels, the story wins.
    pub fn kind_of(&self, card: &Card, current: Kind) -> Kind {
        let story = card.id_labels.contains(&self.story);
        let task = card.id_labels.contains(&self.task);
        match current {
            Kind::Task if story => Kind::Story,
            Kind::Story if task && !story => Kind::Task,
            other => other,
        }
    }
}

pub fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// A Trello card id starts with the moment it was made, in seconds, as eight hex digits.
pub fn card_created_at(id: &str) -> Option<String> {
    let seconds = i64::from_str_radix(id.get(..8)?, 16).ok()?;
    (id.len() == 24 && (1_000_000_000..4_000_000_000).contains(&seconds)).then(|| crate::ids::format_millis(seconds * 1000))
}
