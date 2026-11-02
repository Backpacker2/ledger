//! Turning Claude Code's streaming output (`--output-format stream-json`: one JSON object per line) into the text a
//! person reads while it runs: what it says, which tools it uses, and what went wrong. Tool results (file contents,
//! command output) are left out on purpose: they can be huge, and they may hold things nobody wants copied into a
//! note that is synced. A line that isn't JSON (an error the program printed itself) is shown as it is.

use serde_json::Value;

/// What the final `result` message said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResultInfo {
    pub is_error: bool,
    pub text: String,
    pub turns: Option<u64>,
    /// Tool calls the permission mode refused, as "Tool detail".
    pub denials: Vec<String>,
}

/// One thing the run did, for the record kept in the thread: a tool it used and what it was used on, or a tool
/// call that failed (`failed`, with the reason in `detail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub name: String,
    pub detail: String,
    pub failed: bool,
}

#[derive(Debug, Default)]
pub struct Rendered {
    pub text: String,
    pub result: Option<ResultInfo>,
    pub steps: Vec<Step>,
    /// The model the run started with, from the first message.
    pub model: Option<String>,
}

/// One line of output as text to show (empty when it carries nothing worth showing) and, for the last line, the result.
pub fn render_line(line: &str) -> Rendered {
    let line = line.trim();
    if line.is_empty() {
        return Rendered::default();
    }
    let Some(value) = serde_json::from_str::<Value>(line).ok().filter(|v| v.get("type").is_some()) else {
        return Rendered { text: format!("{line}\n"), ..Rendered::default() };
    };

    let mut out = String::new();
    let mut result = None;
    let mut steps = Vec::new();
    let mut started_with = None;
    match value.get("type").and_then(Value::as_str).unwrap_or_default() {
        "system" => match value.get("subtype").and_then(Value::as_str) {
            Some("init") => {
                let model = value.get("model").and_then(Value::as_str);
                let mode = value.get("permissionMode").and_then(Value::as_str).unwrap_or("?");
                out.push_str(&format!("● Started: {}, permission mode {mode}\n", model.unwrap_or("default model")));
                started_with = model.map(str::to_string);
            }
            Some("api_retry") => {
                let error = value.get("error").and_then(Value::as_str).unwrap_or("error");
                let attempt = value.get("attempt").and_then(Value::as_u64).unwrap_or(0);
                out.push_str(&format!("… The API returned an error ({error}); trying again (attempt {attempt})\n"));
            }
            _ => {}
        },
        "assistant" => {
            for block in content_blocks(&value) {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or_default().trim_end();
                        if !text.is_empty() {
                            out.push_str(text);
                            out.push('\n');
                        }
                    }
                    Some("tool_use") => {
                        let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let detail = block.get("input").map(tool_detail).unwrap_or_default();
                        out.push_str(&format!("→ {name}{}\n", if detail.is_empty() { String::new() } else { format!(" {detail}") }));
                        steps.push(Step { name: name.to_string(), detail, failed: false });
                    }
                    _ => {}
                }
            }
        }
        "user" => {
            for block in content_blocks(&value) {
                if block.get("type").and_then(Value::as_str) == Some("tool_result") && block.get("is_error").and_then(Value::as_bool) == Some(true) {
                    let text = match block.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" "),
                        _ => String::new(),
                    };
                    let reason = shorten(first_line(&text), 200);
                    out.push_str(&format!("  ✗ {reason}\n"));
                    steps.push(Step { name: String::new(), detail: reason, failed: true });
                }
            }
        }
        "result" => {
            let denials = value
                .get("permission_denials")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|d| {
                            let name = d.get("tool_name").and_then(Value::as_str).unwrap_or("tool");
                            let detail = d.get("tool_input").map(tool_detail).unwrap_or_default();
                            if detail.is_empty() { name.to_string() } else { format!("{name} {detail}") }
                        })
                        .collect()
                })
                .unwrap_or_default();
            result = Some(ResultInfo {
                is_error: value.get("is_error").and_then(Value::as_bool).unwrap_or(false),
                text: value.get("result").and_then(Value::as_str).unwrap_or_default().to_string(),
                turns: value.get("num_turns").and_then(Value::as_u64),
                denials,
            });
        }
        _ => {}
    }
    Rendered { text: out, result, steps, model: started_with }
}

fn content_blocks(message: &Value) -> Vec<&Value> {
    match message.pointer("/message/content") {
        Some(Value::Array(blocks)) => blocks.iter().collect(),
        _ => Vec::new(),
    }
}

/// The part of a tool call worth a glance: the file, the command, the pattern.
fn tool_detail(input: &Value) -> String {
    for key in ["file_path", "path", "notebook_path", "command", "pattern", "url", "query", "description"] {
        if let Some(text) = input.get(key).and_then(Value::as_str) {
            return shorten(first_line(text), 120);
        }
    }
    String::new()
}

fn first_line(text: &str) -> &str {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

/// The closing lines of a run: how long it took, and what was refused.
pub fn summary(result: Option<&ResultInfo>, seconds: u64) -> String {
    let mut parts = vec![format!("{}:{:02}", seconds / 60, seconds % 60)];
    let mut out = String::new();
    if let Some(result) = result {
        if let Some(turns) = result.turns {
            parts.push(format!("{turns} turn{}", if turns == 1 { "" } else { "s" }));
        }
        for denied in result.denials.iter().take(10) {
            out.push_str(&format!("⊘ Refused by the permission mode: {}\n", shorten(denied, 160)));
        }
        if result.denials.len() > 10 {
            out.push_str(&format!("⊘ …and {} more refused\n", result.denials.len() - 10));
        }
        if result.is_error && !result.text.trim().is_empty() {
            out.push_str(&format!("✗ {}\n", shorten(first_line(&result.text), 300)));
        }
    }
    out.push_str(&format!("— {}\n", parts.join(" · ")));
    out
}
