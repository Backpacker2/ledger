//! The real [`Remote`](crate::sync::Remote): a branch of a GitHub repository, driven through the REST API
//! ("Git Data" endpoints), so nothing needs to be installed and the access token never leaves this process.
//!
//! The token needs read and write access to the repository's contents (a fine-grained token scoped to just that
//! repository with "Contents: Read and write" is enough). `LEDGER_GITHUB_API` points the client at another API
//! base, for GitHub Enterprise or for testing.

use std::collections::BTreeMap;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::blocking::{Client, Response};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::settings::SyncConfig;
use crate::sync::{Change, Head, Remote};

const DEFAULT_API: &str = "https://api.github.com";

pub struct GithubRemote {
    client: Client,
    api: String,
    repo: String,
    branch: String,
    token: String,
}

/// What the settings screen learns by trying the connection.
pub struct RepoCheck {
    pub private: bool,
    pub can_push: bool,
    pub default_branch: String,
    pub branch_exists: bool,
}

impl GithubRemote {
    pub fn new(config: &SyncConfig) -> Result<GithubRemote> {
        let api = std::env::var("LEDGER_GITHUB_API")
            .ok()
            .filter(|api| !api.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_API.to_string());
        let client = Client::builder()
            .user_agent(concat!("ledger/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| Error::sync(format!("Could not set up the connection: {e}")))?;
        Ok(GithubRemote {
            client,
            api: api.trim_end_matches('/').to_string(),
            repo: config.repo.clone(),
            branch: config.branch.clone(),
            token: config.token.clone(),
        })
    }

    /// Looks at the repository: reachable with this token, private, writable, and whether the branch exists.
    pub fn check(&self) -> Result<RepoCheck> {
        let repo = self.ok(Method::GET, &self.repo_path(""), None)?;
        Ok(RepoCheck {
            private: repo["private"].as_bool().unwrap_or(false),
            can_push: repo["permissions"]["push"].as_bool().unwrap_or(true),
            default_branch: repo["default_branch"].as_str().unwrap_or("main").to_string(),
            branch_exists: self.head()?.is_some(),
        })
    }

    /// Notes about people can be private; this refuses to put them anywhere public.
    pub fn ensure_private(&self) -> Result<()> {
        let repo = self.ok(Method::GET, &self.repo_path(""), None)?;
        if repo["private"].as_bool() == Some(false) {
            return Err(Error::sync(format!(
                "{} is a public repository. Ledger only syncs to private repositories, so your notes aren't published by accident.",
                self.repo
            )));
        }
        Ok(())
    }

    fn repo_path(&self, rest: &str) -> String {
        format!("/repos/{}{rest}", self.repo)
    }

    fn request(&self, method: Method, path: &str, body: Option<&Value>, accept: &str) -> Result<Response> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.api))
            .bearer_auth(&self.token)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().map_err(|e| Error::sync(format!("Could not reach GitHub: {e}")))
    }

    /// Status and JSON body, without judging the status.
    fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<(StatusCode, Value)> {
        let response = self.request(method, path, body, "application/vnd.github+json")?;
        let status = response.status();
        let rate_limited = response.headers().get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()) == Some("0");
        let text = response.text().map_err(|e| Error::sync(format!("Could not read GitHub's answer: {e}")))?;
        let mut json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if rate_limited && status == StatusCode::FORBIDDEN {
            json = json!({ "message": "rate limit exceeded" });
        }
        Ok((status, json))
    }

    /// The JSON body of a successful call; anything else becomes a message a person can act on.
    fn ok(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let (status, json) = self.call(method, path, body)?;
        if status.is_success() { Ok(json) } else { Err(failure(status, &json)) }
    }
}

fn failure(status: StatusCode, body: &Value) -> Error {
    let message = body["message"].as_str().unwrap_or("").to_string();
    let lower = message.to_lowercase();
    match status {
        StatusCode::UNAUTHORIZED => Error::sync("GitHub rejected the access token. Check that it is correct and has not expired."),
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS if lower.contains("rate limit") => {
            Error::sync("GitHub is rate limiting this token. Syncing will resume on its own.")
        }
        StatusCode::FORBIDDEN => Error::sync(
            "The token isn't allowed to do that. It needs read and write access to this repository's contents.",
        ),
        StatusCode::NOT_FOUND => Error::sync(
            "GitHub can't find that repository, branch or file. Check the name, and that the token covers this repository.",
        ),
        StatusCode::CONFLICT if lower.contains("empty") => Error::sync(
            "The repository is empty. Give it a first commit (for example by adding a README on GitHub), then sync again.",
        ),
        StatusCode::UNPROCESSABLE_ENTITY if lower.contains("fast forward") || lower.contains("already exists") => Error::Rejected,
        _ => Error::sync(format!("GitHub answered {status}{}", if message.is_empty() { String::new() } else { format!(": {message}") })),
    }
}

impl Remote for GithubRemote {
    fn head(&self) -> Result<Option<Head>> {
        let (status, json) = self.call(Method::GET, &self.repo_path(&format!("/branches/{}", self.branch)), None)?;
        if status == StatusCode::NOT_FOUND {
            // No such branch — or no such repository, which is worth saying instead.
            self.ok(Method::GET, &self.repo_path(""), None)?;
            return Ok(None);
        }
        if !status.is_success() {
            return Err(failure(status, &json));
        }
        let commit = json["commit"]["sha"].as_str();
        let tree = json["commit"]["commit"]["tree"]["sha"].as_str();
        match (commit, tree) {
            (Some(commit), Some(tree)) => Ok(Some(Head { commit: commit.to_string(), tree: tree.to_string() })),
            _ => Err(Error::sync("GitHub's answer about the branch was not what was expected")),
        }
    }

    fn tree(&self, head: &Head) -> Result<BTreeMap<String, String>> {
        let json = self.ok(Method::GET, &self.repo_path(&format!("/git/trees/{}?recursive=1", head.tree)), None)?;
        if json["truncated"].as_bool() == Some(true) {
            return Err(Error::sync("The repository has too many files to sync through the GitHub API."));
        }
        let mut files = BTreeMap::new();
        for entry in json["tree"].as_array().into_iter().flatten() {
            if entry["type"] == "blob"
                && entry["mode"] != "120000"
                && let (Some(path), Some(sha)) = (entry["path"].as_str(), entry["sha"].as_str())
            {
                files.insert(path.to_string(), sha.to_string());
            }
        }
        Ok(files)
    }

    fn blob(&self, _path: &str, sha: &str) -> Result<Vec<u8>> {
        let response = self.request(Method::GET, &self.repo_path(&format!("/git/blobs/{sha}")), None, "application/vnd.github.raw+json")?;
        let status = response.status();
        if !status.is_success() {
            let body: Value = response.json().unwrap_or(Value::Null);
            return Err(failure(status, &body));
        }
        response.bytes().map(|b| b.to_vec()).map_err(|e| Error::sync(format!("Could not download a file: {e}")))
    }

    fn push(&self, parent: Option<&Head>, changes: &[Change], message: &str) -> Result<Head> {
        let mut entries = Vec::with_capacity(changes.len());
        for change in changes {
            entries.push(match change {
                Change::Put { path, bytes } => match std::str::from_utf8(bytes) {
                    Ok(text) => json!({ "path": path, "mode": "100644", "type": "blob", "content": text }),
                    Err(_) => {
                        let blob = self.ok(
                            Method::POST,
                            &self.repo_path("/git/blobs"),
                            Some(&json!({ "content": STANDARD.encode(bytes), "encoding": "base64" })),
                        )?;
                        json!({ "path": path, "mode": "100644", "type": "blob", "sha": blob["sha"] })
                    }
                },
                Change::Delete { path } => json!({ "path": path, "mode": "100644", "type": "blob", "sha": Value::Null }),
            });
        }

        let mut tree_request = json!({ "tree": entries });
        if let Some(parent) = parent {
            tree_request["base_tree"] = json!(parent.tree);
        }
        let tree = self.ok(Method::POST, &self.repo_path("/git/trees"), Some(&tree_request))?;
        let tree_sha = tree["sha"].as_str().ok_or_else(|| Error::sync("GitHub did not return the new tree"))?.to_string();

        let parents: Vec<&str> = parent.iter().map(|p| p.commit.as_str()).collect();
        let commit = self.ok(
            Method::POST,
            &self.repo_path("/git/commits"),
            Some(&json!({ "message": message, "tree": tree_sha, "parents": parents })),
        )?;
        let commit_sha = commit["sha"].as_str().ok_or_else(|| Error::sync("GitHub did not return the new commit"))?.to_string();

        if parent.is_some() {
            self.ok(
                Method::PATCH,
                &self.repo_path(&format!("/git/refs/heads/{}", self.branch)),
                Some(&json!({ "sha": commit_sha, "force": false })),
            )?;
        } else {
            self.ok(
                Method::POST,
                &self.repo_path("/git/refs"),
                Some(&json!({ "ref": format!("refs/heads/{}", self.branch), "sha": commit_sha })),
            )?;
        }
        Ok(Head { commit: commit_sha, tree: tree_sha })
    }
}
