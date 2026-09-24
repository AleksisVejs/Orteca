//! Chats the user had with `claude` or `codex` outside Orteca, read from the
//! CLIs' own transcripts so one can go with a new task the way an earlier task
//! does: what was asked and the final answer. Headless sessions are left out -
//! Orteca's own runs are those, and they are already in its history.

use super::ProviderId;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const LISTED: usize = 30;
// ponytail: fixed cuts so a pasted log does not ride along whole; a per-chat
// token budget if these turn out wrong.
const SAID_CAP: usize = 2_000;
const ANSWER_CAP: usize = 8_000;

/// Where each CLI keeps its transcripts. `None` is a CLI with no home here.
pub struct Homes {
    pub claude: Option<PathBuf>,
    pub codex: Option<PathBuf>,
}

impl Homes {
    pub fn of_user() -> Self {
        let home = |var: &str, dir: &str| {
            std::env::var_os(var)
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(dir)))
        };
        Homes { claude: home("CLAUDE_CONFIG_DIR", ".claude"), codex: home("CODEX_HOME", ".codex") }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub provider: ProviderId,
    pub id: String,
    pub title: String,
    pub turns: usize,
    pub updated_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub title: String,
    pub said: Vec<String>,
    pub answer: String,
}

/// The newest chats started in `root` or a folder inside it, both CLIs together.
pub fn list(root: &Path, homes: &Homes) -> Vec<ChatSummary> {
    let root = norm(&root.to_string_lossy());
    let titles = codex_titles(homes);
    files(&root, homes)
        .into_iter()
        .filter_map(|(provider, path, updated_ms)| {
            let (id, chat) = parse(provider, &path, &root, &titles)?;
            Some(ChatSummary { provider, id, title: chat.title, turns: chat.said.len(), updated_ms })
        })
        .take(LISTED)
        .collect()
}

/// One chat `list` would show. The id picks among files found for `root`, so
/// it never names a path.
pub fn read(root: &Path, homes: &Homes, provider: ProviderId, id: &str) -> Option<Chat> {
    let root = norm(&root.to_string_lossy());
    let titles = codex_titles(homes);
    files(&root, homes)
        .into_iter()
        .filter(|f| f.0 == provider)
        .find_map(|(_, path, _)| parse(provider, &path, &root, &titles).filter(|(found, _)| found == id))
        .map(|(_, chat)| chat)
}

/// Every transcript that could belong to `root`, newest first.
fn files(root: &str, homes: &Homes) -> Vec<(ProviderId, PathBuf, u64)> {
    let mut out = Vec::new();
    if let Some(home) = &homes.claude {
        // Claude names a project's folder after its path, every other character a
        // dash. Lossy, so each chat's own `cwd` still decides; a prefix also
        // takes in chats started in a subfolder.
        let prefix: String = root.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        for dir in std::fs::read_dir(home.join("projects")).into_iter().flatten().flatten() {
            if dir.file_name().to_string_lossy().to_lowercase().starts_with(&prefix) {
                collect(&dir.path(), ProviderId::Claude, false, &mut out);
            }
        }
    }
    if let Some(home) = &homes.codex {
        collect(&home.join("sessions"), ProviderId::Codex, true, &mut out);
    }
    out.sort_by(|a, b| b.2.cmp(&a.2));
    out
}

fn collect(dir: &Path, provider: ProviderId, deep: bool, out: &mut Vec<(ProviderId, PathBuf, u64)>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if deep {
                collect(&path, provider, deep, out);
            }
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok());
            out.push((provider, path, modified.map_or(0, |d| d.as_millis() as u64)));
        }
    }
}

fn parse(provider: ProviderId, path: &Path, root: &str, titles: &HashMap<String, String>) -> Option<(String, Chat)> {
    let (id, title, said, answer) = match provider {
        ProviderId::Claude => claude(path, root)?,
        ProviderId::Codex => codex(path, root, titles)?,
    };
    let first = said.first()?;
    let title = title.unwrap_or_else(|| first.clone());
    let title = cap(title.lines().next().unwrap_or("").trim(), 80);
    let said = said.iter().map(|s| cap(s, SAID_CAP)).collect();
    Some((id, Chat { title, said, answer: cap(&answer, ANSWER_CAP) }))
}

type Parsed = (String, Option<String>, Vec<String>, String);

fn claude(path: &Path, root: &str) -> Option<Parsed> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let (mut title, mut said, mut answer, mut placed) = (None, Vec::new(), String::new(), false);
    for line in lines(path) {
        // Most lines are tool output; only these three kinds are worth parsing.
        if !["\"type\":\"user\"", "\"type\":\"assistant\"", "\"type\":\"custom-title\""].iter().any(|k| line.contains(k)) {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        match v["type"].as_str() {
            Some("custom-title") => title = v["customTitle"].as_str().map(String::from),
            Some(kind @ ("user" | "assistant")) if v["isSidechain"] != true && v["isMeta"] != true => {
                if v["entrypoint"].as_str().is_some_and(|e| e.starts_with("sdk")) {
                    return None;
                }
                if let Some(cwd) = v["cwd"].as_str().filter(|_| !placed) {
                    if !inside(cwd, root) {
                        return None;
                    }
                    placed = true;
                }
                let text = text_of(&v["message"]["content"]);
                if text.is_empty() {
                    continue;
                }
                if kind == "assistant" {
                    answer = text;
                // Slash commands and their output come back wrapped in tags.
                } else if !text.starts_with('<') && !text.starts_with("[Request interrupted") {
                    said.push(text);
                }
            }
            _ => {}
        }
    }
    placed.then_some((id, title, said, answer))
}

fn codex(path: &Path, root: &str, titles: &HashMap<String, String>) -> Option<Parsed> {
    let mut lines = lines(path);
    let meta: Value = serde_json::from_str(&lines.next()?).ok()?;
    let p = &meta["payload"];
    if meta["type"] != "session_meta" || p["originator"] == "codex_exec" || p["source"] == "exec" || !inside(p["cwd"].as_str()?, root) {
        return None;
    }
    let id = p["id"].as_str()?.to_string();
    let (mut said, mut answer) = (Vec::new(), String::new());
    for line in lines {
        if !line.contains("\"type\":\"event_msg\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let e = &v["payload"];
        // Older CLIs log `user_message`, newer ones a completed `UserMessage` item.
        let text = match e["type"].as_str() {
            Some("user_message") => e["message"].as_str().unwrap_or("").trim().to_string(),
            Some("item_completed") if e["item"]["type"] == "UserMessage" => text_of(&e["item"]["content"]),
            Some("task_complete") => {
                if let Some(last) = e["last_agent_message"].as_str() {
                    answer = last.trim().to_string();
                }
                continue;
            }
            _ => continue,
        };
        if !text.is_empty() {
            said.push(text);
        }
    }
    Some((id.clone(), titles.get(&id).cloned(), said, answer))
}

/// Codex keeps each thread's name apart from its transcript.
fn codex_titles(homes: &Homes) -> HashMap<String, String> {
    let Some(home) = &homes.codex else { return HashMap::new() };
    lines(&home.join("session_index.jsonl"))
        .filter_map(|line| {
            let v: Value = serde_json::from_str(&line).ok()?;
            Some((v["id"].as_str()?.to_string(), v["thread_name"].as_str()?.to_string()))
        })
        .collect()
}

fn lines(path: &Path) -> impl Iterator<Item = String> {
    std::fs::File::open(path)
        .into_iter()
        .flat_map(|f| BufReader::new(f).lines().map_while(Result::ok))
}

/// A message's words: a plain string, or the text parts of a content list.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

fn cap(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_string(),
    }
}

fn norm(p: &str) -> String {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p);
    p.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn inside(cwd: &str, root: &str) -> bool {
    let cwd = norm(cwd);
    cwd == root || cwd.strip_prefix(root).is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: PathBuf, lines: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: Vec<String> = lines.iter().map(|v| v.to_string()).collect();
        std::fs::write(path, text.join("\n")).unwrap();
    }

    #[test]
    fn reads_both_clis_and_skips_headless_and_other_folders() {
        let base = std::env::temp_dir().join(format!("orteca-chats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (claude, codex) = (base.join("claude"), base.join("codex"));
        let root = r"C:\Work\Shop";
        let user = |text: &str, cwd: &str, entry: &str| {
            serde_json::json!({"type":"user","cwd":cwd,"entrypoint":entry,"message":{"role":"user","content":text}})
        };
        let said = |text: &str| serde_json::json!({"type":"assistant","message":{"content":[{"type":"text","text":text}]}});

        write(claude.join("projects/C--Work-Shop/aaa.jsonl"), &[
            user("add a cart", root, "cli"),
            serde_json::json!({"type":"user","cwd":root,"message":{"content":[{"type":"tool_result","content":"ok"}]}}),
            user("<command-name>/clear</command-name>", root, "cli"),
            said("Cart added."),
            user("make it red", root, "cli"),
            said("Now red."),
            serde_json::json!({"type":"custom-title","customTitle":"Cart work"}),
        ]);
        // Orteca's own run, and a sibling folder whose name encodes the same.
        write(claude.join("projects/C--Work-Shop/bbb.jsonl"), &[user("x", root, "sdk-cli"), said("y")]);
        write(claude.join("projects/C--Work-Shop-old/ccc.jsonl"), &[user("x", r"C:\Work\Shop-old", "cli"), said("y")]);

        let meta = |id: &str, originator: &str| {
            serde_json::json!({"type":"session_meta","payload":{"id":id,"cwd":r"C:\Work\Shop\app","originator":originator}})
        };
        let event = |payload: Value| serde_json::json!({"type":"event_msg","payload":payload});
        write(codex.join("sessions/2026/09/23/rollout-a.jsonl"), &[
            meta("t1", "Codex Desktop"),
            event(serde_json::json!({"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"fix the footer"}]}})),
            event(serde_json::json!({"type":"task_complete","last_agent_message":"Footer fixed."})),
        ]);
        write(codex.join("sessions/2026/09/23/rollout-b.jsonl"), &[
            meta("t2", "codex_exec"),
            event(serde_json::json!({"type":"user_message","message":"x"})),
        ]);
        std::fs::write(codex.join("session_index.jsonl"), r#"{"id":"t1","thread_name":"Footer"}"#).unwrap();

        let homes = Homes { claude: Some(claude), codex: Some(codex) };
        let root = Path::new(r"\\?\C:\Work\Shop");
        let mut found: Vec<_> = list(root, &homes).into_iter().map(|c| (c.id, c.title, c.turns)).collect();
        found.sort();
        assert_eq!(found, [("aaa".into(), "Cart work".into(), 2), ("t1".into(), "Footer".into(), 1)]);

        let chat = read(root, &homes, ProviderId::Claude, "aaa").unwrap();
        assert_eq!((chat.said, chat.answer.as_str()), (vec!["add a cart".to_string(), "make it red".into()], "Now red."));
        assert_eq!(read(root, &homes, ProviderId::Codex, "t1").unwrap().answer, "Footer fixed.");
        assert!(read(root, &homes, ProviderId::Claude, "bbb").is_none());
        assert!(read(root, &homes, ProviderId::Codex, "aaa").is_none());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
