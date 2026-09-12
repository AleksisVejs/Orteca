//! Codex `exec --json` events.
//!
//! Codex reports tokens and no cost at all. Every usage event it produces is
//! `unavailable`; inventing a dollar figure from published rates would be the
//! invented precision this project refuses to ship.

use serde_json::Value;

use super::{CostQuality, FailureKind, ProviderEvent, Usage};

pub fn parse_line(v: &Value) -> Vec<ProviderEvent> {
    match v["type"].as_str().unwrap_or_default() {
        "thread.started" => v["thread_id"]
            .as_str()
            .map(|id| {
                vec![ProviderEvent::Started {
                    session_id: id.to_string(),
                }]
            })
            .unwrap_or_default(),
        "item.completed" => item(&v["item"]).into_iter().collect(),
        "turn.completed" => {
            let u = &v["usage"];
            vec![
                ProviderEvent::Usage(Usage {
                    input_tokens: n(&u["input_tokens"]),
                    cached_input_tokens: n(&u["cached_input_tokens"]),
                    output_tokens: n(&u["output_tokens"]),
                    reasoning_tokens: n(&u["reasoning_output_tokens"]),
                    cost_usd: None,
                    cost_quality: CostQuality::Unavailable,
                }),
                // Codex has no final-answer field; the caller keeps the last Text.
                ProviderEvent::Done {
                    result: String::new(),
                    structured: None,
                },
            ]
        }
        "turn.failed" => vec![ProviderEvent::Failed {
            kind: FailureKind::Crashed,
            message: message_of(&v["error"]),
        }],
        "error" => vec![ProviderEvent::Failed {
            kind: FailureKind::Crashed,
            message: message_of(v),
        }],
        _ => Vec::new(),
    }
}

fn item(i: &Value) -> Option<ProviderEvent> {
    match i["type"].as_str()? {
        "agent_message" => Some(ProviderEvent::Text(i["text"].as_str()?.to_string())),
        "command_execution" => Some(ProviderEvent::ToolUse {
            name: "Shell".into(),
            summary: i["command"].as_str().unwrap_or_default().to_string(),
        }),
        "file_change" => Some(ProviderEvent::ToolUse {
            name: "Edit".into(),
            summary: changed_paths(&i["changes"]),
        }),
        "error" => Some(ProviderEvent::Failed {
            kind: FailureKind::Crashed,
            message: message_of(i),
        }),
        _ => None,
    }
}

fn changed_paths(changes: &Value) -> String {
    changes
        .as_array()
        .map(|c| {
            c.iter()
                .filter_map(|c| c["path"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn message_of(v: &Value) -> String {
    v["message"]
        .as_str()
        .or_else(|| v.as_str())
        .unwrap_or("Codex failed without a message")
        .to_string()
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}
