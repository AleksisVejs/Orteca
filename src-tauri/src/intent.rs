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
    /// Wants an answer, not a change.
    Question,
    Easy,
    Medium,
    Hard,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reading {
    pub intent: Option<Intent>,
    pub title: String,
    pub job: Option<Job>,
}

// One line: it travels as an argument, and a Windows shim mangles newlines.
const INSTRUCTION: &str = "You classify requests made to a coding agent working in a git repository. Reply with exactly four lines and nothing else. Line 1 is one label: question, easy, medium, or hard. question means no file changes; easy is narrow; medium is ordinary work across a few files; hard is cross-cutting or design-heavy. Line 2 is a title of at most six words in the request's language, no quotes. Line 3 starts `job:` followed by a comma-separated subset of general, security, authentication, authorization, schema. Use security only for a security boundary or vulnerability; authentication for login/session/credential behavior; authorization for access or permission behavior; schema for a database/schema migration. Display text, translations, documentation, or styling that merely mentions auth, login, roles, permissions, or database is general. Input validation, pagination or page-size limits, rate limits, and other resource bounds are general unless the request is about an authentication or permission boundary. Line 4 starts `run:` followed by a comma-separated subset of build, test, lint, or none. Include an action only when the user explicitly asks to run it. A request that asks a question and also asks for a change is not a question. The request may be in any language.";

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
    args_with(id, INSTRUCTION)
}

fn args_with(id: ProviderId, instruction: &str) -> Vec<String> {
    let model = model(id);
    let fixed: Vec<&str> = match id {
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
        ],
        ProviderId::Codex => [
            &["exec", "-", "--json"][..],
            CODEX_ISOLATION,
            &[
                "--model",
                model,
                "-c",
                "model_reasoning_effort=\"low\"",
                "--sandbox",
                "read-only",
                "--skip-git-repo-check",
                "--ephemeral",
            ],
        ]
        .concat(),
    };
    fixed.into_iter().map(str::to_string).collect()
}

/// The longest title kept. The sidebar cuts it to fit with an ellipsis.
pub const TITLE_CHARS: usize = 60;

/// The first of the four words the reply contains, wherever it sits.
pub fn parse(reply: &str) -> Option<Intent> {
    reply
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphabetic())
        .find_map(|word| match word {
            "question" => Some(Intent::Question),
            "easy" => Some(Intent::Easy),
            "medium" => Some(Intent::Medium),
            "hard" => Some(Intent::Hard),
            _ => None,
        })
}

/// The reply's second line, stripped of markdown, quotes and a "Title:"
/// label. Empty when the model gave none; the sidebar then shows the prompt.
pub fn parse_title(reply: &str) -> String {
    let line = reply.lines().map(str::trim).filter(|l| !l.is_empty()).nth(1).unwrap_or("");
    let line = line.trim_start_matches(['#', '*', '-', '>', ' ']);
    let line = line
        .split_once(':')
        .filter(|(label, _)| label.trim().eq_ignore_ascii_case("title"))
        .map_or(line, |(_, rest)| rest);
    let title = line.trim_matches(['*', '_', '`', '"', '\'', '“', '”', '.', ' ']);
    title.chars().take(TITLE_CHARS).collect()
}

fn values(reply: &str, label: &str, allowed: &[&str]) -> Option<Vec<String>> {
    let value = reply.lines().map(str::trim).find_map(|line| {
        let (found, value) = line.split_once(':')?;
        found.trim().eq_ignore_ascii_case(label).then_some(value)
    })?;
    let values: Vec<String> = value
        .split(',')
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    (!values.is_empty()
        && values
            .iter()
            .all(|value| value == "none" || allowed.contains(&value.as_str())))
    .then_some(values)
}

pub fn parse_job(reply: &str) -> Option<Job> {
    let kinds = values(
        reply,
        "job",
        &["general", "security", "authentication", "authorization", "schema"],
    )?;
    let actions = values(reply, "run", &["build", "test", "lint"])?;
    Some(Job {
        security: kinds.iter().any(|value| value == "security"),
        authz: kinds
            .iter()
            .any(|value| matches!(value.as_str(), "authentication" | "authorization")),
        schema_change: kinds.iter().any(|value| value == "schema"),
        build: actions.iter().any(|value| value == "build"),
        test: actions.iter().any(|value| value == "test"),
        lint: actions.iter().any(|value| value == "lint"),
    })
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

async fn propose_rules(id: ProviderId, program: &str, instruction: &str, request: &str) -> Vec<String> {
    let mut events = Vec::new();
    let reply = tokio::time::timeout(DEADLINE, ask_with(id, program, instruction, request, &mut events))
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

/// Ask the provider what `prompt` wants and what to call it, in one call.
/// `None` means "could not tell" and the keyword router decides; the events
/// come back either way, so what the call cost is still counted.
pub async fn read(
    id: ProviderId,
    program: &str,
    prompt: &str,
) -> (Reading, Vec<ProviderEvent>) {
    let mut events = Vec::new();
    let reply = tokio::time::timeout(DEADLINE, ask(id, program, prompt, &mut events))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    (
        Reading {
            intent: parse(&reply),
            title: parse_title(&reply),
            job: parse_job(&reply),
        },
        events,
    )
}

async fn ask(
    id: ProviderId,
    program: &str,
    prompt: &str,
    events: &mut Vec<ProviderEvent>,
) -> Option<String> {
    ask_with(id, program, INSTRUCTION, &format!("Request:\n{prompt}"), events).await
}

async fn ask_with(
    id: ProviderId,
    program: &str,
    instruction: &str,
    request: &str,
    events: &mut Vec<ProviderEvent>,
) -> Option<String> {
    let argv = args_with(id, instruction);
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    // Never in the user's project: nobody has consented to its settings for this.
    let mut run = proc::spawn_env(program, &borrowed, &std::env::temp_dir(), &env(id)).ok()?;
    let request = match id {
        ProviderId::Claude => request.to_string(),
        ProviderId::Codex => format!("{instruction}\n\n{request}"),
    };
    run.send_line(&request).await.ok()?;
    run.close_stdin();
    let mut reply = String::new();
    while let Some(line) = run.lines.recv().await {
        match line {
            Line::Json(v) => {
                for event in id.parse_line(&v) {
                    match &event {
                        ProviderEvent::Text(text) => reply = text.clone(),
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
        assert!(args_with(ProviderId::Claude, MEMORY_INSTRUCTION).join(" ").contains("--tools  --system-prompt"));
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

    #[test]
    fn reads_the_first_label_in_the_reply() {
        assert_eq!(parse("question"), Some(Intent::Question));
        assert_eq!(parse("Hard."), Some(Intent::Hard));
        assert_eq!(parse("**easy**\n"), Some(Intent::Easy));
        assert_eq!(parse("It's medium, not hard"), Some(Intent::Medium));
        assert_eq!(parse("I cannot tell"), None);
        assert_eq!(parse("uneasy"), None);
    }

    #[test]
    fn reads_the_title_from_the_second_line() {
        assert_eq!(parse_title("easy\nFix login redirect"), "Fix login redirect");
        assert_eq!(parse_title("**hard**\n\nTitle: \"Rework the router.\""), "Rework the router");
        assert_eq!(parse_title("- medium\n# 2FA setup page"), "2FA setup page");
        assert_eq!(parse_title("question"), "");
        assert_eq!(parse_title(&format!("easy\n{}", "x".repeat(200))).len(), TITLE_CHARS);
    }

    #[test]
    fn reads_job_risk_and_only_explicit_requested_actions() {
        assert_eq!(
            parse_job("easy\nFix login copy\njob: general\nrun: build"),
            Some(Job {
                build: true,
                ..Default::default()
            })
        );
        assert_eq!(
            parse_job("medium\nProtect delete\njob: security, authorization\nrun: test, lint"),
            Some(Job {
                security: true,
                authz: true,
                test: true,
                lint: true,
                ..Default::default()
            })
        );
        assert_eq!(parse_job("easy\nFix copy"), None);
        assert_eq!(parse_job("easy\nFix copy\njob: mystery\nrun: none"), None);
    }

    #[test]
    fn classifier_calls_have_no_tools_and_no_write_access() {
        let claude = args(ProviderId::Claude).join(" ");
        assert!(claude.contains("--tools  --system-prompt"));
        assert!(!claude.contains("--bare"));
        let codex = args(ProviderId::Codex).join(" ");
        assert!(codex.contains("--sandbox read-only"));
        assert!(!codex.contains("danger-full-access"));
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
