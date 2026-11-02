//! Talking to Trello. The API sits behind the [`Trello`] trait so that the sync can be tested against a fake
//! one with in-memory boards; [`HttpTrello`] is the real one, driven through the REST API.
//!
//! Credentials: the API key and token go in an `Authorization: OAuth oauth_consumer_key="…", oauth_token="…"`
//! header and nowhere else: never in a URL, a query, a body, an error message or a log. They are only ever
//! handled by [`ReqwestWire`]; everything above it works with requests that carry no credentials at all.
//!
//! Rate limits: Trello allows about 100 requests per 10 seconds per token. Requests are paced (never closer
//! than [`MIN_GAP`]), a 429 is waited out for as long as Trello's `Retry-After` says and then retried, and a
//! server error or a dropped connection is retried with a growing pause. When that doesn't help, the caller
//! gets [`TrelloError::RateLimited`] and backs off for longer. `LEDGER_TRELLO_API` points the client at
//! another API base, for testing.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::header::{HeaderValue, AUTHORIZATION};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Error;

const DEFAULT_API: &str = "https://api.trello.com/1";

/// Never send two requests closer together than this: about 8 a second, safely under Trello's 100 per 10 seconds.
pub const MIN_GAP: Duration = Duration::from_millis(120);
const MAX_ATTEMPTS: usize = 5;
/// The longest single pause honoured for a 429; a longer `Retry-After` is passed on to the caller instead.
const MAX_WAIT: Duration = Duration::from_secs(60);

const CARD_FIELDS: &str = "name,desc,idList,idBoard,idLabels,idMembers,closed,dateLastActivity,idShort";

// ---- what Trello holds --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Member {
    pub id: String,
    pub username: String,
    pub full_name: String,
}

impl Member {
    /// "Ana Smith (@ana)", for messages.
    pub fn display(&self) -> String {
        match (self.full_name.trim(), self.username.trim()) {
            ("", "") => self.id.clone(),
            ("", user) => format!("@{user}"),
            (name, "") => name.to_string(),
            (name, user) => format!("{name} (@{user})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Board {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct List {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Card {
    pub id: String,
    pub id_short: i64,
    pub name: String,
    pub desc: String,
    pub id_list: String,
    pub id_board: String,
    pub id_labels: Vec<String>,
    pub id_members: Vec<String>,
    pub closed: bool,
    /// ISO timestamp of the card's latest activity (an edit, a move, a comment).
    pub date_last_activity: String,
}

pub struct NewCard {
    pub list: String,
    pub name: String,
    pub desc: String,
    /// The one member the card is assigned to.
    pub member: String,
    pub labels: Vec<String>,
}

/// Only what is present changes.
#[derive(Debug, Default, Clone)]
pub struct CardUpdate {
    pub name: Option<String>,
    pub desc: Option<String>,
    pub list: Option<String>,
}

impl CardUpdate {
    pub fn is_empty(&self) -> bool {
        self.name.is_none() && self.desc.is_none() && self.list.is_none()
    }
}

// ---- errors -------------------------------------------------------------------------------------------

#[derive(Debug)]
pub enum TrelloError {
    /// Trello kept answering 429 (or asked for a longer wait than is worth blocking for).
    RateLimited { retry_after: Duration },
    /// The key or token was rejected: nothing else will work either, so a sync stops.
    Unauthorized,
    /// Something a person can read and act on.
    Message(String),
    /// The vault side of a sync failed.
    Ledger(Error),
}

pub type TResult<T> = std::result::Result<T, TrelloError>;

impl TrelloError {
    pub fn message(text: impl Into<String>) -> Self {
        TrelloError::Message(text.into())
    }
}

impl fmt::Display for TrelloError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrelloError::RateLimited { .. } => f.write_str("Trello is rate limiting this token. Syncing will resume on its own."),
            TrelloError::Unauthorized => f.write_str(
                "Trello rejected the API key or token. Check that both are correct and that the token hasn't expired or been revoked.",
            ),
            TrelloError::Message(text) => f.write_str(text),
            TrelloError::Ledger(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TrelloError {}

impl From<Error> for TrelloError {
    fn from(e: Error) -> Self {
        TrelloError::Ledger(e)
    }
}

impl From<TrelloError> for Error {
    fn from(e: TrelloError) -> Self {
        match e {
            TrelloError::Ledger(e) => e,
            other => Error::sync(other.to_string()),
        }
    }
}

// ---- the API ------------------------------------------------------------------------------------------

pub trait Trello: Sync {
    /// The account the credentials belong to.
    fn me(&self) -> TResult<Member>;
    /// The account's open boards.
    fn boards(&self) -> TResult<Vec<Board>>;
    fn board_members(&self, board: &str) -> TResult<Vec<Member>>;
    /// The board's open lists, left to right.
    fn lists(&self, board: &str) -> TResult<Vec<List>>;
    fn labels(&self, board: &str) -> TResult<Vec<Label>>;
    fn create_label(&self, board: &str, name: &str, color: &str) -> TResult<Label>;
    /// Every open card on the board, whoever it belongs to.
    fn cards(&self, board: &str) -> TResult<Vec<Card>>;
    /// One card by id, wherever it is now; `None` once it no longer exists.
    fn card(&self, id: &str) -> TResult<Option<Card>>;
    fn create_card(&self, new: &NewCard) -> TResult<Card>;
    fn update_card(&self, id: &str, update: &CardUpdate) -> TResult<Card>;
}

// ---- the wire -----------------------------------------------------------------------------------------

/// A request without credentials: those are added by the wire.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub body: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    /// From the `Retry-After` header, when it is a number of seconds.
    pub retry_after: Option<Duration>,
    pub body: String,
}

/// Sends one request. `Err` is a failure to get an answer at all, in words fit for a person (no credentials).
pub trait Wire: Sync {
    fn send(&self, request: &Request) -> std::result::Result<Reply, String>;
}

/// The header value for Trello's OAuth-style authorization.
pub fn auth_header(key: &str, token: &str) -> String {
    format!("OAuth oauth_consumer_key=\"{key}\", oauth_token=\"{token}\"")
}

/// Whether a key or token can be sent in a header as it is: visible ASCII, without the characters that would
/// break the header's quoting.
pub fn valid_credential(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '\\' | ','))
}

pub struct ReqwestWire {
    client: Client,
    api: String,
    auth: HeaderValue,
}

impl ReqwestWire {
    pub fn new(key: &str, token: &str) -> std::result::Result<ReqwestWire, TrelloError> {
        let api = std::env::var("LEDGER_TRELLO_API")
            .ok()
            .filter(|api| !api.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_API.to_string());
        let mut auth = HeaderValue::from_str(&auth_header(key, token))
            .map_err(|_| TrelloError::message("The API key or token contains characters that can't be sent. Copy them again from Trello."))?;
        auth.set_sensitive(true);
        let client = Client::builder()
            .user_agent(concat!("ledger/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| TrelloError::message(format!("Could not set up the connection: {e}")))?;
        Ok(ReqwestWire { client, api: api.trim_end_matches('/').to_string(), auth })
    }
}

impl Wire for ReqwestWire {
    fn send(&self, request: &Request) -> std::result::Result<Reply, String> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).map_err(|e| e.to_string())?;
        let mut builder = self
            .client
            .request(method, format!("{}{}", self.api, request.path))
            .header(AUTHORIZATION, self.auth.clone())
            .header("Accept", "application/json")
            .query(&request.query);
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        // `without_url`: the address is harmless (the credentials are in a header), but there is no reason to repeat it.
        let response = builder.send().map_err(|e| format!("Could not reach Trello: {}", e.without_url()))?;
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);
        let body = response.text().map_err(|e| format!("Could not read Trello's answer: {}", e.without_url()))?;
        Ok(Reply { status, retry_after, body })
    }
}

// ---- the client ---------------------------------------------------------------------------------------

pub type Sleeper = Arc<dyn Fn(Duration) + Send + Sync>;

pub struct HttpTrello<W: Wire> {
    wire: W,
    sleep: Sleeper,
    last: Mutex<Option<Instant>>,
}

impl HttpTrello<ReqwestWire> {
    pub fn new(key: &str, token: &str) -> TResult<Self> {
        Ok(HttpTrello::with_wire(ReqwestWire::new(key, token)?, Arc::new(std::thread::sleep)))
    }
}

impl<W: Wire> HttpTrello<W> {
    /// `sleep` is how pauses are made; tests pass one that only records them.
    pub fn with_wire(wire: W, sleep: Sleeper) -> Self {
        HttpTrello { wire, sleep, last: Mutex::new(None) }
    }

    /// Waits until at least [`MIN_GAP`] has passed since the previous request.
    fn pace(&self) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = *last {
            let elapsed = at.elapsed();
            if elapsed < MIN_GAP {
                (self.sleep)(MIN_GAP - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    /// Sends the request, waiting out rate limits and retrying server errors. Returns the status and the JSON body
    /// (a body that isn't JSON comes back as a string) of the first answer that is neither.
    fn send(&self, request: &Request) -> TResult<(u16, Value)> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            self.pace();
            let last = attempt >= MAX_ATTEMPTS;
            match self.wire.send(request) {
                Err(e) => {
                    if last {
                        return Err(TrelloError::Message(e));
                    }
                    (self.sleep)(backoff(attempt));
                }
                Ok(reply) if reply.status == 429 => {
                    let wait = reply.retry_after.unwrap_or_else(|| backoff(attempt));
                    if last || wait > MAX_WAIT {
                        return Err(TrelloError::RateLimited { retry_after: wait });
                    }
                    (self.sleep)(wait);
                }
                Ok(reply) if (500..600).contains(&reply.status) => {
                    if last {
                        return Err(TrelloError::message(format!("Trello is having trouble right now (it answered {}). Syncing will try again.", reply.status)));
                    }
                    (self.sleep)(backoff(attempt));
                }
                Ok(reply) => {
                    let body = serde_json::from_str(&reply.body).unwrap_or_else(|_| Value::String(reply.body.trim().to_string()));
                    return Ok((reply.status, body));
                }
            }
        }
    }

    /// The JSON body of a successful call; anything else becomes a message a person can act on.
    fn ok(&self, request: Request) -> TResult<Value> {
        let (status, body) = self.send(&request)?;
        if (200..300).contains(&status) { Ok(body) } else { Err(failure(status, &body)) }
    }

    fn get(&self, path: impl Into<String>, query: &[(&str, &str)]) -> TResult<Value> {
        self.ok(Request {
            method: "GET",
            path: path.into(),
            query: query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            body: None,
        })
    }

    fn write(&self, method: &'static str, path: impl Into<String>, body: Value) -> TResult<Value> {
        self.ok(Request { method, path: path.into(), query: vec![], body: Some(body) })
    }
}

fn backoff(attempt: usize) -> Duration {
    Duration::from_secs(1u64 << (attempt - 1).min(5))
}

/// What a refusal means, as words for the settings screen. The body is Trello's own plain text.
fn failure(status: u16, body: &Value) -> TrelloError {
    let detail = match body {
        Value::String(text) => text.clone(),
        other => other["message"].as_str().unwrap_or_default().to_string(),
    };
    let detail: String = detail.chars().take(200).collect();
    match status {
        401 => TrelloError::Unauthorized,
        403 => TrelloError::message("Trello says this token isn't allowed to do that. Generate a token with read and write access."),
        404 => TrelloError::message("Trello can't find that board, list or card (it may have been deleted, or the token can't see it)."),
        400 => TrelloError::message(if detail.is_empty() { "Trello refused the request.".to_string() } else { format!("Trello refused the request: {detail}") }),
        _ => TrelloError::message(format!("Trello answered {status}{}", if detail.is_empty() { String::new() } else { format!(": {detail}") })),
    }
}

fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> TResult<T> {
    serde_json::from_value(value).map_err(|_| TrelloError::message("Trello's answer was not what was expected."))
}

impl<W: Wire> Trello for HttpTrello<W> {
    fn me(&self) -> TResult<Member> {
        parse(self.get("/members/me", &[("fields", "username,fullName")])?)
    }

    fn boards(&self) -> TResult<Vec<Board>> {
        parse(self.get("/members/me/boards", &[("filter", "open"), ("fields", "name")])?)
    }

    fn board_members(&self, board: &str) -> TResult<Vec<Member>> {
        parse(self.get(format!("/boards/{board}/members"), &[("fields", "username,fullName")])?)
    }

    fn lists(&self, board: &str) -> TResult<Vec<List>> {
        parse(self.get(format!("/boards/{board}/lists"), &[("filter", "open"), ("fields", "name")])?)
    }

    fn labels(&self, board: &str) -> TResult<Vec<Label>> {
        parse(self.get(format!("/boards/{board}/labels"), &[("fields", "name,color"), ("limit", "1000")])?)
    }

    fn create_label(&self, board: &str, name: &str, color: &str) -> TResult<Label> {
        parse(self.write("POST", format!("/boards/{board}/labels"), json!({ "name": name, "color": color }))?)
    }

    fn cards(&self, board: &str) -> TResult<Vec<Card>> {
        let cards: Vec<Card> = parse(self.get(format!("/boards/{board}/cards/open"), &[("fields", CARD_FIELDS)])?)?;
        Ok(cards.into_iter().filter(|c| !c.closed).collect())
    }

    fn card(&self, id: &str) -> TResult<Option<Card>> {
        let (status, body) = self.send(&Request {
            method: "GET",
            path: format!("/cards/{id}"),
            query: vec![("fields".into(), CARD_FIELDS.into())],
            body: None,
        })?;
        match status {
            404 => Ok(None),
            200..=299 => parse(body).map(Some),
            _ => Err(failure(status, &body)),
        }
    }

    fn create_card(&self, new: &NewCard) -> TResult<Card> {
        parse(self.write(
            "POST",
            "/cards",
            json!({
                "idList": new.list,
                "name": new.name,
                "desc": new.desc,
                "idMembers": new.member,
                "idLabels": new.labels.join(","),
                "pos": "bottom",
            }),
        )?)
    }

    fn update_card(&self, id: &str, update: &CardUpdate) -> TResult<Card> {
        let mut body = serde_json::Map::new();
        if let Some(name) = &update.name {
            body.insert("name".into(), json!(name));
        }
        if let Some(desc) = &update.desc {
            body.insert("desc".into(), json!(desc));
        }
        if let Some(list) = &update.list {
            body.insert("idList".into(), json!(list));
        }
        parse(self.write("PUT", format!("/cards/{id}"), Value::Object(body))?)
    }
}
