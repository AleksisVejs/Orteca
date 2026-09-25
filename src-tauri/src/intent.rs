//! What the user is asking for, read by the provider's smallest model.
//!
//! Keywords cannot tell "should I email a user who cancelled?" from "email a
//! user who cancelled", and they only speak English. One small call can, in any
//! language. Its answer only picks a route: the keyword gates in `routing` still
//! escalate security, schema and twice-failed work, and never step down.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::proc::{self, Line};
use crate::providers::{ProviderEvent, ProviderId, CODEX_ISOLATION};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Intent {
    /// Talk, not a question about the code: "i love Leina" once sent the
    /// Answer stage searching the repo for her name.
    Chat,
    /// Wants an answer, not a change.
    Question,
    Easy,
    Medium,
    Hard,
}

/// What the task is, beside how hard (`Intent`). It picks the ruleset and the
/// tools; the difficulty still picks the route and the tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Chat,
    Question,
    #[default]
    CodeChange,
    Debug,
    Plan,
}

impl TaskType {
    pub const ALL: [TaskType; 5] = [Self::Chat, Self::Question, Self::CodeChange, Self::Debug, Self::Plan];

    pub fn name(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Question => "question",
            Self::CodeChange => "code_change",
            Self::Debug => "debug",
            Self::Plan => "plan",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The route this type runs on: a plan is written like an answer, read-only.
    pub fn intent(self, difficulty: Option<Intent>) -> Option<Intent> {
        match self {
            Self::Chat => Some(Intent::Chat),
            Self::Question | Self::Plan => Some(Intent::Question),
            Self::CodeChange | Self::Debug => difficulty,
        }
    }

    /// Where a wrong guess at what was meant wastes a whole run.
    pub fn may_clarify(self) -> bool {
        matches!(self, Self::CodeChange | Self::Debug | Self::Plan)
    }
}

/// What kind of work the small classifier read, and which repository-declared
/// commands the user explicitly asked Orteca to run. `None` at the call site
/// means classification failed and the deterministic fallback takes over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Job {
    pub security: bool,
    pub authz: bool,
    pub schema_change: bool,
    pub build: bool,
    pub test: bool,
    pub lint: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    pub intent: Option<Intent>,
    pub title: String,
    pub job: Option<Job>,
    pub task_type: Option<TaskType>,
    /// How sure the classifier was of the type, 0 to 1.
    pub confidence: Option<f64>,
    /// One question to ask before a run a wrong guess would waste.
    pub clarify: Option<String>,
    /// Talk, already answered in the same call.
    pub reply: Option<String>,
    /// The latest message asks for files to change. Read on its own because
    /// the type alone mixes "try it, it crashes" with "fix it": both debug.
    pub change: Option<bool>,
    /// It asks why something happened or broke: a cause to trace, which the
    /// cheapest tier answered "No" to three times out of three.
    pub cause: Option<bool>,
}

/// Below this the type falls back to the broader one: a question, not talk.
const SURE: f64 = 0.6;

// One line: it travels as an argument, and a Windows shim mangles newlines.
const INSTRUCTION: &str = "You classify one request made to a coding agent working in a git repository. You run in an empty scratch folder, not in that repository, so its files are not here; the agent will have them. The request may be in any language. Fill every field. type: chat is small talk or a personal message with nothing to look up; question wants an answer and no file changes; plan wants a plan to approve before any change; debug wants something broken found and fixed; code_change is any other change. A request that asks a question and also asks for a change is not a question. difficulty: easy is narrow; medium is ordinary work across a few files; hard is cross-cutting or design-heavy. confidence: from 0 to 1, how sure you are of the type. clarify: null, unless the type is code_change, debug or plan and the request itself leaves out what to change, holds conflicting requirements, or gives no way to tell when it is done, so that even an agent that reads the whole repository could not tell what the user wants; then one short question in the request's language about that missing part. The agent finds any file, page, project or existing code on its own, so never ask about those. title: at most six words in the request's language. job: security only for a security boundary or vulnerability; authentication for login, session or credential behavior; authorization for access or permission behavior; schema for a database or schema migration; otherwise general. Display text, translations, documentation or styling that merely mention these are general, and so are input validation, pagination, rate limits and other resource bounds unless the request is about an authentication or permission boundary. run: build, test or lint only when the user explicitly asks to run it. reply: for chat only, your answer in the request's language in at most three short sentences; otherwise null. change: true only when the latest message asks for files to be changed or something to be fixed, or accepts a change an answer proposed; asking to check, try, reproduce, investigate or explain something is false. cause: true when the latest message asks why something happened or does not work, whether something caused a problem, or what is broken; false for how-to, where-is, what-does-this-do and every other question. When the request has `My reply:`, classify only that reply; the answer before it is context, so a reply like `do that` means the change that answer proposed. When a previous type is given, keep it unless the reply asks for something different.";

/// The reading's shape, enforced by `claude --json-schema` and
/// `codex --output-schema`. Every field required, as Codex's strict mode wants.
const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["type","difficulty","confidence","clarify","title","job","run","reply","change","cause"],"properties":{"change":{"type":"boolean"},"cause":{"type":"boolean"},"type":{"type":"string","enum":["chat","question","code_change","debug","plan"]},"difficulty":{"type":"string","enum":["easy","medium","hard"]},"confidence":{"type":"number"},"clarify":{"type":["string","null"]},"title":{"type":"string"},"job":{"type":"array","items":{"type":"string","enum":["general","security","authentication","authorization","schema"]}},"run":{"type":"array","items":{"type":"string","enum":["build","test","lint"]}},"reply":{"type":["string","null"]}}}"#;

/// A classifier slower than this costs more waiting than it can save. The
/// wait is the model, not the shim: the CLI itself starts in ~0.3s, and an
/// unbounded haiku spent 22-61s and up to 6.2k tokens on one classification
/// (2026-09-19). `THINKING` is what keeps the call inside this.
const DEADLINE: Duration = Duration::from_secs(45);

/// The ceiling a Claude classifier call reasons under.
///
/// Codex has always had one, as `model_reasoning_effort` in `args`. Claude
/// had none, so haiku reasoned without a ceiling and the whole route waited:
/// on one prompt 22s, 37s and 61s on three runs, the last past `DEADLINE`.
/// Bounded, the same three ran 12-15s and returned the same label every time.
/// `--effort low` is not this bound - a run under it still reached 4.7k
/// tokens. Removing the reasoning is not either: at zero, one prompt read as
/// `easy`, `security`, `hard` and `medium` on four runs.
// ponytail: 1024 is the first value tried, on two prompts and one machine.
const THINKING: &str = "1024";

/// The environment a classifier call needs on top of the inherited one.
fn env(id: ProviderId) -> Vec<(&'static str, std::path::PathBuf)> {
    match id {
        ProviderId::Claude => vec![("MAX_THINKING_TOKENS", THINKING.into())],
        // Bounded by `model_reasoning_effort` on the command line instead.
        ProviderId::Codex => Vec::new(),
    }
}

/// The model and effort each CLI is asked for.
pub fn model(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Claude => "haiku",
        ProviderId::Codex => "gpt-6-luna",
    }
}

/// The normal classifier argv is only asserted by unit tests. Runtime calls
/// supply their instruction explicitly through `args_with`.
#[cfg(test)]
fn args(id: ProviderId) -> Vec<String> {
    args_with(id, INSTRUCTION, Some(SCHEMA))
}

/// Where a Codex call's instruction or schema is kept, named by its content
/// so a file once written is never rewritten under a call reading it.
fn temp_file(text: &str, ext: &str) -> std::path::PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hash);
    std::env::temp_dir().join(format!("orteca-instruction-{:016x}.{ext}", hash.finish()))
}

fn args_with(id: ProviderId, instruction: &str, schema: Option<&str>) -> Vec<String> {
    let model = model(id);
    // Codex's own system prompt is ~7k tokens of agent guidance a one-line
    // answer never uses; this replaces it. A TOML string, so `\` is escaped.
    let instructions = format!(
        "model_instructions_file=\"{}\"",
        temp_file(instruction, "txt").to_string_lossy().replace('\\', "\\\\")
    );
    let schema_file = schema.map(|s| temp_file(s, "json").to_string_lossy().into_owned());
    let mut fixed: Vec<&str> = match id {
        // No tools and a replaced system prompt: the whole call is the
        // instruction and the request. The prompt arrives on stdin.
        ProviderId::Claude => vec![
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--setting-sources",
            "project,local",
            "--strict-mcp-config",
            "--disable-slash-commands",
            "--no-session-persistence",
            "--tools",
            "",
            "--system-prompt",
            instruction,
            "--model",
            model,
            "--fallback-model",
            "sonnet",
        ],
        ProviderId::Codex => [
            &["exec", "-", "--json"][..],
            CODEX_ISOLATION,
            &[
                "--model",
                model,
                "-c",
                "model_reasoning_effort=\"low\"",
                // A classifier once searched the web before answering.
                "-c",
                "web_search=\"disabled\"",
                "-c",
                &instructions,
                "--sandbox",
                "read-only",
                "--skip-git-repo-check",
                "--ephemeral",
            ],
        ]
        .concat(),
    };
    match (id, schema, schema_file.as_deref()) {
        (ProviderId::Claude, Some(schema), _) => fixed.extend(["--json-schema", schema]),
        (ProviderId::Codex, _, Some(file)) => fixed.extend(["--output-schema", file]),
        _ => {}
    }
    fixed.into_iter().map(str::to_string).collect()
}

/// The longest title kept. The sidebar cuts it to fit with an ellipsis.
pub const TITLE_CHARS: usize = 60;

/// The classifier's JSON, or `None` when it is not the shape asked for. Low
/// confidence in talk reads as a question, and only talk keeps a reply.
pub fn parse_reading(reply: &str) -> Option<Reading> {
    let v: serde_json::Value = serde_json::from_str(reply.trim()).ok()?;
    let mut task_type = TaskType::from_name(v["type"].as_str()?)?;
    let confidence = v["confidence"].as_f64();
    if task_type == TaskType::Chat && confidence.is_some_and(|c| c < SURE) {
        task_type = TaskType::Question;
    }
    let difficulty = match v["difficulty"].as_str() {
        Some("easy") => Some(Intent::Easy),
        Some("medium") => Some(Intent::Medium),
        Some("hard") => Some(Intent::Hard),
        _ => None,
    };
    let words = |key: &str| -> Vec<String> {
        v[key].as_array().into_iter().flatten().filter_map(|w| w.as_str()).map(str::to_string).collect()
    };
    let (kinds, runs) = (words("job"), words("run"));
    let has = |list: &[String], w: &str| list.iter().any(|x| x == w);
    let text = |key: &str| v[key].as_str().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
    Some(Reading {
        intent: task_type.intent(difficulty),
        title: text("title").unwrap_or_default().trim_matches(['"', '.', ' ']).chars().take(TITLE_CHARS).collect(),
        job: Some(Job {
            security: has(&kinds, "security"),
            authz: has(&kinds, "authentication") || has(&kinds, "authorization"),
            schema_change: has(&kinds, "schema"),
            build: has(&runs, "build"),
            test: has(&runs, "test"),
            lint: has(&runs, "lint"),
        }),
        task_type: Some(task_type),
        confidence,
        clarify: text("clarify").filter(|_| task_type.may_clarify()),
        reply: text("reply").filter(|_| task_type == TaskType::Chat),
        change: v["change"].as_bool(),
        cause: v["cause"].as_bool(),
    })
}

/// A `/command` at the start picks the type and skips the classifier.
pub fn command(text: &str) -> Option<(TaskType, String)> {
    let text = text.trim_start();
    let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let task_type = match word {
        "/chat" => TaskType::Chat,
        "/ask" | "/question" => TaskType::Question,
        "/code" => TaskType::CodeChange,
        "/debug" | "/fix" => TaskType::Debug,
        "/plan" => TaskType::Plan,
        _ => return None,
    };
    Some((task_type, rest.trim().to_string()))
}

const MEMORY_INSTRUCTION: &str = "You turn a personal instructions file for a coding agent into short standing rules. Reply with only a list, one rule per line, each line starting with `- `. Each rule is one plain sentence of at most 20 words that keeps the file's meaning. Keep only lasting rules about how the agent should behave or write code. Skip headings, examples, explanations, and anything that is not an instruction. Do not add rules the file does not state. At most 25 rules.";

/// The rules a small model proposes from an instructions file. Nothing is
/// saved here; the user approves each one first.
pub async fn memory_rules(id: ProviderId, program: &str, file: &str) -> Vec<String> {
    propose_rules(id, program, MEMORY_INSTRUCTION, &format!("Instructions file:\n{file}")).await
}

const RUNS_INSTRUCTION: &str = "You read a coding agent's recent runs in one project and propose short standing rules that would have saved the user effort. Reply with only a list, one rule per line, each line starting with `- `. Each rule is one plain sentence of at most 20 words. Propose a rule only for what the runs show more than once: a correction the user repeated, a convention they asked for, a check that kept failing. Never restate one task, and never repeat a rule already saved. At most 8 rules; an empty reply is fine.";

/// Rules a small model proposes from a digest of this project's runs, minus
/// the ones already saved. Nothing is saved here either.
pub async fn run_rules(id: ProviderId, program: &str, digest: &str, saved: &[String]) -> Vec<String> {
    let saved = if saved.is_empty() { "none".to_string() } else { saved.join("\n") };
    let request = format!("Rules already saved:\n{saved}\n\nRecent runs, newest first:\n{digest}");
    propose_rules(id, program, RUNS_INSTRUCTION, &request).await
}

const COMMIT_INSTRUCTION: &str = "You write a git commit message for the change you are given. First line: an imperative summary of the whole change, at most 72 characters, no trailing period, no prefix like feat:. Then a blank line, then one line per distinct change starting with `- `: a plain sentence saying what changed and why, naming the area it touches. Cover every file in the file list, including files whose patch was cut; group small related edits into one line. Reply with the message only, no quotes and no code fences.";

/// The most of a patch the small model reads. The rest is cut, not summarised,
/// but every file still reaches it by name.
// ponytail: a byte cap, not a token count; a huge change is described past the cut from file names only.
const COMMIT_PATCH_CHARS: usize = 40_000;

/// A commit message - subject, blank line, bullets - a small model drafts from
/// `patch`. Empty when it gave none; the user edits it before anything is
/// committed either way.
pub async fn commit_message(id: ProviderId, program: &str, patch: &str) -> String {
    let mut events = Vec::new();
    let reply = tokio::time::timeout(DEADLINE, ask_with(id, program, COMMIT_INSTRUCTION, None, &commit_request(patch), &mut events))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    parse_commit(&reply)
}

/// Every changed file by name, then as much of the patch as fits.
fn commit_request(patch: &str) -> String {
    let files: Vec<&str> = patch
        .lines()
        .filter_map(|l| l.strip_prefix("diff --git ")?.rsplit_once(" b/").map(|(_, b)| b))
        .collect();
    let cut: String = patch.chars().take(COMMIT_PATCH_CHARS).collect();
    let note = if cut.len() < patch.len() { " (cut short; describe the rest from the file list)" } else { "" };
    format!("Files changed ({}):\n{}\n\nPatch{note}:\n{cut}", files.len(), files.join("\n"))
}

fn parse_commit(reply: &str) -> String {
    let lines: Vec<&str> = reply.lines().map(str::trim_end).filter(|l| !l.trim_start().starts_with("```")).collect();
    let text = lines.join("\n");
    let text = text.trim();
    let (subject, body) = text.split_once('\n').unwrap_or((text, ""));
    let subject = subject.trim_matches(['*', '_', '`', '"', '\'', ' ']);
    match body.trim() {
        "" => subject.to_string(),
        body => format!("{subject}\n\n{body}"),
    }
}

async fn propose_rules(id: ProviderId, program: &str, instruction: &str, request: &str) -> Vec<String> {
    let mut events = Vec::new();
    let reply = tokio::time::timeout(DEADLINE, ask_with(id, program, instruction, None, request, &mut events))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    parse_rules(&reply)
}

fn parse_rules(reply: &str) -> Vec<String> {
    reply
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- ").or_else(|| l.trim().strip_prefix("* ")))
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .take(25)
        .collect()
}

/// The smallest call there is, on the cheapest model: it starts the plan's
/// rolling window when the user wants it started. Whether it answered, and
/// what it cost.
pub async fn ping(id: ProviderId, program: &str) -> (bool, Option<crate::providers::Usage>) {
    let mut events = Vec::new();
    let reply = tokio::time::timeout(DEADLINE, ask_with(id, program, "Reply with the word ok.", None, "ok?", &mut events))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let usage = events.into_iter().find_map(|e| match e {
        ProviderEvent::Usage(u) => Some(u),
        _ => None,
    });
    (!reply.trim().is_empty(), usage)
}

/// Ask the provider what `prompt` wants and what to call it, in one call.
/// `None` fields mean "could not tell" and the keyword router decides; the
/// events come back either way, so what the call cost is still counted.
/// `previous` is the type of the task a follow-up continues.
pub async fn read(
    id: ProviderId,
    program: &str,
    prompt: &str,
    previous: Option<TaskType>,
) -> (Reading, Vec<ProviderEvent>) {
    let mut events = Vec::new();
    let request = match previous {
        Some(t) => format!("Previous type: {}\nRequest:\n{prompt}", t.name()),
        None => format!("Request:\n{prompt}"),
    };
    let reply = tokio::time::timeout(DEADLINE, ask_with(id, program, INSTRUCTION, Some(SCHEMA), &request, &mut events))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    (parse_reading(&reply).unwrap_or_default().after(previous), events)
}

impl Reading {
    /// Asking why something broke, or following up a question, without asking
    /// for a change is a question. "Can you try it yourself? It will crash"
    /// read as debug on every try and ran a $2 change route on a user who had
    /// said "just tell me"; Claude read "what's causing the worker to use more
    /// memory?" as debug 3 times in 3, and a crash report as talk, which reads
    /// no file.
    pub fn after(mut self, previous: Option<TaskType>) -> Self {
        let cause = self.cause == Some(true);
        let no_change = self.change == Some(false)
            && matches!(self.task_type, Some(TaskType::Debug | TaskType::CodeChange))
            && (cause || previous == Some(TaskType::Question));
        if no_change || (cause && self.task_type == Some(TaskType::Chat)) {
            self.task_type = Some(TaskType::Question);
            self.intent = TaskType::Question.intent(None);
            (self.clarify, self.reply) = (None, None);
        }
        self
    }
}

async fn ask_with(
    id: ProviderId,
    program: &str,
    instruction: &str,
    schema: Option<&str>,
    request: &str,
    events: &mut Vec<ProviderEvent>,
) -> Option<String> {
    if id == ProviderId::Codex {
        for (text, ext) in std::iter::once((instruction, "txt")).chain(schema.map(|s| (s, "json"))) {
            let file = temp_file(text, ext);
            if !file.exists() {
                std::fs::write(&file, text).ok()?;
            }
        }
    }
    let argv = args_with(id, instruction, schema);
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    // Never in the user's project: nobody has consented to its settings for this.
    let mut run = proc::spawn_env(program, &borrowed, &std::env::temp_dir(), &env(id)).ok()?;
    run.send_line(request).await.ok()?;
    run.close_stdin();
    let mut reply = String::new();
    while let Some(line) = run.lines.recv().await {
        match line {
            Line::Json(v) => {
                // Every call carries the plan's usage: keep it, for free.
                crate::providers::limits::remember(id, &v);
                for event in id.parse_line(&v) {
                    match &event {
                        ProviderEvent::Text(text) => reply = text.clone(),
                        // A schema's answer arrives as data, not as words.
                        ProviderEvent::Done { structured: Some(value), .. } => reply = value.to_string(),
                        ProviderEvent::Done { result, .. } if !result.is_empty() => {
                            reply = result.clone()
                        }
                        _ => {}
                    }
                    events.push(event);
                }
            }
            Line::Exit(_) => break,
            _ => {}
        }
    }
    Some(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rules_only_from_list_lines() {
        assert_eq!(parse_rules("Here:
- Use tabs.
* Be brief
-
not a rule"), ["Use tabs.", "Be brief"]);
        assert!(args_with(ProviderId::Claude, MEMORY_INSTRUCTION, None).join(" ").contains("--tools  --system-prompt"));
    }

    #[test]
    fn reads_the_commit_subject_and_body() {
        assert_eq!(parse_commit("\n`Add commit drafts`\n"), "Add commit drafts");
        assert_eq!(
            parse_commit("```\n\"Add commit drafts\"\n\n- Draft a body\n- List every file\n```"),
            "Add commit drafts\n\n- Draft a body\n- List every file"
        );
        assert_eq!(parse_commit(""), "");
        // A file past the cut still reaches the model by name.
        let patch = format!("diff --git a/a.rs b/a.rs\n+{}\ndiff --git a/src/b c.ts b/src/b c.ts\n+x\n", "y".repeat(COMMIT_PATCH_CHARS));
        let request = commit_request(&patch);
        assert!(request.starts_with("Files changed (2):\na.rs\nsrc/b c.ts\n"));
        assert!(request.contains("cut short"));
    }

    /// Spends a few cents: proposes rules from the real ~/.claude/CLAUDE.md.
    /// `cargo test live_memory_rules -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_memory_rules() {
        let file = std::fs::read_to_string(std::path::Path::new(&std::env::var("USERPROFILE").unwrap()).join(".claude/CLAUDE.md")).unwrap();
        let program = crate::providers::which("claude").unwrap();
        let rules = memory_rules(ProviderId::Claude, &program.to_string_lossy(), &file).await;
        println!("{rules:#?}");
        assert!(!rules.is_empty());
    }

    /// Spends a few tiny calls: reads prompts with each CLI and prints what
    /// came back and what it cost. LIVE_PROVIDER picks claude or codex.
    /// `cargo test live_classify -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_classify() {
        let id: ProviderId = serde_json::from_value(serde_json::Value::String(
            std::env::var("LIVE_PROVIDER").unwrap_or_else(|_| "claude".into()),
        ))
        .unwrap();
        let program = crate::providers::which(id.program()).unwrap();
        for prompt in ["thanks, that was great!", "why does the login page redirect twice?", "fix it", "Pievieno tumšo režīmu iestatījumu lapai"] {
            let started = std::time::Instant::now();
            let (reading, events) = read(id, &program.to_string_lossy(), prompt, None).await;
            let usage = events.iter().find_map(|e| match e {
                ProviderEvent::Usage(u) => Some(u.clone()),
                _ => None,
            });
            println!("{prompt:?} -> {:?} conf {:?} clarify {:?} reply {:?} title {:?} | {:?} in {:?}", reading.task_type, reading.confidence, reading.clarify, reading.reply, reading.title, usage.map(|u| (u.input_tokens, u.cached_input_tokens, u.output_tokens, u.cost_usd)), started.elapsed());
            assert!(reading.task_type.is_some(), "no reading for {prompt:?}");
        }
    }

    /// Spends a little. First questions, each read `LIVE_RUNS` times (default
    /// 3): a cause to trace or not. A miss either way costs: a cheap wrong "No",
    /// or a dear answer to "where is the login route?".
    /// `LIVE_PROVIDER=codex cargo test live_cause -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn live_cause_questions() {
        let id: ProviderId = serde_json::from_value(serde_json::Value::String(
            std::env::var("LIVE_PROVIDER").unwrap_or_else(|_| "claude".into()),
        ))
        .unwrap();
        let program = crate::providers::which(id.program()).unwrap().to_string_lossy().into_owned();
        let runs: usize = std::env::var("LIVE_RUNS").ok().and_then(|n| n.parse().ok()).unwrap_or(3);
        use TaskType::*;
        const ASK: &[TaskType] = &[Question];
        const WORK: &[TaskType] = &[Debug, CodeChange];
        // The type `read` ends on, and the tier signal where it matters.
        let cases: &[(&str, &[TaskType], Option<bool>)] = &[
            ("i wrote \"fix this and that\" in orteca but when it was about to answer it closed the task, i believe it has to be something to do with a follow-up question from the AI but it crashed, just tell me if that is the case.", ASK, Some(true)),
            ("why does the login page redirect twice?", ASK, Some(true)),
            ("why is the build failing on CI but not on my machine?", ASK, Some(true)),
            ("is the csv export broken? the files look wrong", ASK, Some(true)),
            ("what's causing the worker to use more memory every hour?", ASK, Some(true)),
            ("my tests passed yesterday and now 3 fail, what changed?", ASK, Some(true)),
            ("kāpēc iestatījumu lapa ielādējas divreiz?", ASK, Some(true)),
            ("how do I run the tests?", ASK, Some(false)),
            ("where is the login route defined?", ASK, Some(false)),
            ("what does routing.rs do?", ASK, Some(false)),
            ("explain the architecture simply", ASK, Some(false)),
            ("is this the best app for a claude+codex coding harness right now?", &[Question, Chat], Some(false)),
            ("which model does the review stage use?", ASK, Some(false)),
            ("kā palaist šo projektu lokāli?", ASK, Some(false)),
            // Work stays work: a cause asked with a fix, or a bug to fix.
            ("why does the login page redirect twice? fix it", WORK, None),
            ("the export button crashes the app when the table is empty", WORK, None),
            ("fix the failing test in scripts/app.test.mjs", WORK, None),
            ("the build is failing on CI, find out why and fix it", WORK, None),
            ("salabo, lai iestatījumu lapa neielādējas divreiz", WORK, None),
            ("add a dark mode toggle to the settings page", &[CodeChange], None),
        ];
        // Seven at a time: 42 at once all ran past the classifier's deadline.
        let asks: Vec<_> = cases.iter().flat_map(|&c| std::iter::repeat_n(c, runs)).collect();
        let mut readings = Vec::new();
        for batch in asks.chunks(7) {
            let calls: Vec<_> = batch
                .iter()
                .map(|&(prompt, ok, cause)| {
                    let program = program.clone();
                    tokio::spawn(async move { (prompt, ok, cause, read(id, &program, prompt, None).await.0) })
                })
                .collect();
            for call in calls {
                readings.push(call.await.unwrap());
            }
        }
        let mut tally: Vec<(&str, usize, Vec<String>)> = Vec::new();
        for (prompt, ok, cause, reading) in readings {
            let right = reading.task_type.is_some_and(|t| ok.contains(&t))
                && cause.is_none_or(|c| reading.cause == Some(c));
            let got = format!("{:?}/{:?}", reading.task_type, reading.cause);
            match tally.iter_mut().find(|(p, ..)| *p == prompt) {
                Some(row) => {
                    row.1 += usize::from(right);
                    row.2.push(got);
                }
                None => tally.push((prompt, usize::from(right), vec![got])),
            }
        }
        let mut failed = Vec::new();
        for (prompt, right, got) in &tally {
            println!("{id:?} | {right}/{} {:.60}: {got:?}", got.len(), prompt);
            if right * 2 <= got.len() {
                failed.push(*prompt);
            }
        }
        assert!(failed.is_empty(), "wrong on most runs: {failed:?}");
    }

    /// After a question, only a reply that asks for a change leaves the question.
    #[test]
    fn a_follow_up_to_a_question_changes_nothing_unless_it_asks_to() {
        let read_as = |t: &str, change: bool, cause: bool, previous: Option<TaskType>| {
            parse_reading(&format!(
                r#"{{"type":"{t}","difficulty":"medium","confidence":0.9,"clarify":"Which?","title":"t","job":["general"],"run":[],"reply":"hi","change":{change},"cause":{cause}}}"#
            ))
            .unwrap()
            .after(previous)
        };
        let read = |t: &str, change: bool, previous: Option<TaskType>| read_as(t, change, false, previous);
        // Why something broke, with no change asked for, is a question on a first ask too.
        assert_eq!(read_as("debug", false, true, None).task_type, Some(TaskType::Question));
        assert_eq!(read_as("debug", true, true, None).task_type, Some(TaskType::Debug));
        // A bug report that asks nothing and names no cause keeps its type.
        assert_eq!(read_as("debug", false, false, None).task_type, Some(TaskType::Debug));
        // A cause is looked up in the files, which talk never opens.
        let talk = read_as("chat", false, true, None);
        assert_eq!((talk.task_type, talk.reply), (Some(TaskType::Question), None));
        assert_eq!(read_as("chat", false, false, None).task_type, Some(TaskType::Chat));
        let tried = read("debug", false, Some(TaskType::Question));
        assert_eq!((tried.task_type, tried.intent, tried.clarify), (Some(TaskType::Question), Some(Intent::Question), None));
        assert_eq!(read("debug", true, Some(TaskType::Question)).task_type, Some(TaskType::Debug));
        assert_eq!(read("code_change", true, Some(TaskType::Question)).task_type, Some(TaskType::CodeChange));
        // Anywhere else the type stands: a debug task stays debug while it is tried.
        assert_eq!(read("debug", false, Some(TaskType::Debug)).task_type, Some(TaskType::Debug));
        assert_eq!(read("debug", false, None).task_type, Some(TaskType::Debug));
    }

    #[test]
    fn reads_the_classifier_json() {
        let json = |t: &str, c: f64, clarify: &str, reply: &str| format!(
            r#"{{"type":"{t}","difficulty":"medium","confidence":{c},"clarify":{clarify},"title":"Protect delete.","job":["security","authorization"],"run":["test"],"reply":{reply}}}"#
        );
        let r = parse_reading(&json("debug", 0.9, r#""Which page?""#, r#""hi""#)).unwrap();
        assert_eq!((r.task_type, r.intent), (Some(TaskType::Debug), Some(Intent::Medium)));
        assert_eq!(r.title, "Protect delete");
        assert_eq!(r.job, Some(Job { security: true, authz: true, test: true, ..Default::default() }));
        assert_eq!((r.clarify.as_deref(), r.reply), (Some("Which page?"), None), "only talk keeps a reply");
        let plan = parse_reading(&json("plan", 0.9, "null", "null")).unwrap();
        assert_eq!(plan.intent, Some(Intent::Question), "a plan runs read-only");
        let chat = parse_reading(&json("chat", 0.9, r#""x""#, r#""Thanks!""#)).unwrap();
        assert_eq!((chat.reply.as_deref(), chat.clarify), (Some("Thanks!"), None));
        let unsure = parse_reading(&json("chat", 0.3, "null", r#""Thanks!""#)).unwrap();
        assert_eq!((unsure.task_type, unsure.reply), (Some(TaskType::Question), None), "unsure talk is a question");
        assert_eq!(parse_reading("easy
Fix it"), None);
    }

    #[test]
    fn a_command_picks_the_type() {
        assert_eq!(command("/plan add dark mode"), Some((TaskType::Plan, "add dark mode".into())));
        assert_eq!(command("/debug"), Some((TaskType::Debug, String::new())));
        assert_eq!(command("/usr/bin is slow"), None);
    }

    #[test]
    fn classifier_calls_have_no_tools_and_no_write_access() {
        let claude = args(ProviderId::Claude).join(" ");
        assert!(claude.contains("--tools  --system-prompt"));
        assert!(!claude.contains("--bare"));
        let codex = args(ProviderId::Codex).join(" ");
        assert!(codex.contains("--sandbox read-only"));
        assert!(!codex.contains("danger-full-access"));
        assert!(codex.contains("web_search=\"disabled\""));
        assert!(codex.contains(r"\\orteca-instruction-"), "the path is a TOML string: {codex}");
        assert!(codex.contains("--output-schema") && claude.contains("--json-schema"));
    }

    /// Both providers bound the reasoning, by the means each one has. Without
    /// it the call runs past `DEADLINE` and the route waits for nothing.
    #[test]
    fn every_classifier_call_bounds_its_reasoning() {
        let claude = env(ProviderId::Claude);
        assert_eq!(claude, vec![("MAX_THINKING_TOKENS", THINKING.into())]);
        assert!(env(ProviderId::Codex).is_empty());
        assert!(args(ProviderId::Codex).join(" ").contains("model_reasoning_effort=\"low\""));
    }
}
