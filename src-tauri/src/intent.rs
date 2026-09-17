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
const INSTRUCTION: &str = "You classify requests made to a coding agent working in a git repository. Reply with exactly four lines and nothing else. Line 1 is one label: question, easy, medium, or hard. question means no file changes; easy is narrow; medium is ordinary work across a few files; hard is cross-cutting or design-heavy. Line 2 is a title of at most six words in the request's language, no quotes. Line 3 starts `job:` followed by a comma-separated subset of general, security, authentication, authorization, schema. Use security only for a security boundary or vulnerability; authentication for login/session/credential behavior; authorization for access or permission behavior; schema for a database/schema migration. Display text, translations, documentation, or styling that merely mentions auth, login, roles, permissions, or database is general. Line 4 starts `run:` followed by a comma-separated subset of build, test, lint, or none. Include an action only when the user explicitly asks to run it. A request that asks a question and also asks for a change is not a question. The request may be in any language.";

/// A small model's first minute is mostly a Node shim starting. A classifier
/// slower than this costs more waiting than it can save.
const DEADLINE: Duration = Duration::from_secs(45);

/// The model and effort each CLI is asked for.
pub fn model(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Claude => "haiku",
        ProviderId::Codex => "gpt-5.6-luna",
    }
}

fn args(id: ProviderId) -> Vec<String> {
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
            INSTRUCTION,
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
    let argv = args(id);
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    // Never in the user's project: nobody has consented to its settings for this.
    let mut run = proc::spawn(program, &borrowed, &std::env::temp_dir()).ok()?;
    let request = match id {
        ProviderId::Claude => format!("Request:\n{prompt}"),
        ProviderId::Codex => format!("{INSTRUCTION}\n\nRequest:\n{prompt}"),
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
}
