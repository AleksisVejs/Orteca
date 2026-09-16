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

// One line: it travels as an argument, and a Windows shim mangles newlines.
const INSTRUCTION: &str = "You sort requests made to a coding agent working in a git repository. Reply with exactly one word and nothing else. question: it wants an answer, advice or an explanation, and no file changed. easy: a small, narrow change. medium: ordinary work across a few files. hard: large, cross-cutting or design-heavy work. A request that asks something and also asks for a change is not a question. The request may be in any language.";

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

/// Ask the provider what `prompt` wants. `None` means "could not tell" and the
/// keyword router decides; the events come back either way, so what the call
/// cost is still counted.
pub async fn read(id: ProviderId, program: &str, prompt: &str) -> (Option<Intent>, Vec<ProviderEvent>) {
    let mut events = Vec::new();
    let intent = tokio::time::timeout(DEADLINE, ask(id, program, prompt, &mut events))
        .await
        .ok()
        .flatten();
    (intent, events)
}

async fn ask(
    id: ProviderId,
    program: &str,
    prompt: &str,
    events: &mut Vec<ProviderEvent>,
) -> Option<Intent> {
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
    parse(&reply)
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
    fn classifier_calls_have_no_tools_and_no_write_access() {
        let claude = args(ProviderId::Claude).join(" ");
        assert!(claude.contains("--tools  --system-prompt"));
        assert!(!claude.contains("--bare"));
        let codex = args(ProviderId::Codex).join(" ");
        assert!(codex.contains("--sandbox read-only"));
        assert!(!codex.contains("danger-full-access"));
    }
}
