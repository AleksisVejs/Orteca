//! Codex `exec --json` events.
//!
//! Codex reports tokens and no cost at all. Every usage event it produces is
//! `unavailable`; inventing a dollar figure from published rates would be the
//! invented precision this project refuses to ship.

use serde_json::Value;

use super::{classify_failure, CostQuality, ProviderEvent, Usage};

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
                    // `exec --json` 0.154.0 names no model in any event.
                    model: None,
                    input_tokens: n(&u["input_tokens"]).saturating_sub(n(&u["cached_input_tokens"])),
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
                    turns: 1,
                },
            ]
        }
        "turn.failed" => vec![failed(message_of(&v["error"]))],
        "error" => vec![failed(message_of(v))],
        _ => Vec::new(),
    }
}

/// Codex's `--output-schema` constrains the final agent message, but the JSONL
/// stream still carries that value in the message's `text` field. Accept the
/// explicit structured fields used by newer clients as well, so upgrading the
/// CLI does not silently turn artifacts back into prose.
pub fn structured_output(v: &Value) -> Option<Value> {
    match v["type"].as_str().unwrap_or_default() {
        "turn.completed" => ["structured_output", "structuredOutput", "output", "result"]
            .iter()
            .find_map(|key| json_value(v.get(*key))),
        "item.completed" => {
            let item = &v["item"];
            if !matches!(item["type"].as_str(), Some("agent_message" | "output_text")) {
                return None;
            }
            ["structured_output", "structuredOutput", "output", "text"]
                .iter()
                .find_map(|key| json_value(item.get(*key)))
        }
        _ => None,
    }
}

fn json_value(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    match value {
        Value::Object(_) | Value::Array(_) => Some(value.clone()),
        Value::String(text) => serde_json::from_str(text).ok(),
        _ => None,
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
        "error" => Some(failed(message_of(i))),
        _ => None,
    }
}

fn failed(message: String) -> ProviderEvent {
    ProviderEvent::Failed {
        kind: classify_failure(&message),
        message,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_reads_are_not_counted_twice() {
        let events = parse_line(&serde_json::json!({"type":"turn.completed", "usage": {
            "input_tokens": 100, "cached_input_tokens": 80, "output_tokens": 20, "reasoning_output_tokens": 5
        }}));
        let ProviderEvent::Usage(u) = &events[0] else { panic!("missing usage") };
        assert_eq!(u.input_tokens + u.cached_input_tokens + u.output_tokens, 120);
        assert_eq!(u.model, None, "codex names no model, so none is invented");
    }

    #[test]
    fn schema_constrained_agent_message_is_exposed_as_structured_output() {
        let line = serde_json::json!({
            "type": "item.completed",
            "item": {
                "type": "agent_message",
                "text": "{\"objective\":\"rename it\",\"constraints\":[],\"affected_areas\":[],\"implementation_steps\":[],\"risks\":[],\"tests_required\":[]}"
            }
        });
        assert_eq!(
            structured_output(&line),
            Some(serde_json::json!({
                "objective": "rename it",
                "constraints": [],
                "affected_areas": [],
                "implementation_steps": [],
                "risks": [],
                "tests_required": []
            }))
        );
    }

    #[test]
    fn ordinary_codex_text_is_not_an_artifact() {
        let line = serde_json::json!({
            "type": "item.completed",
            "item": { "type": "agent_message", "text": "I checked the change." }
        });
        assert_eq!(structured_output(&line), None);
    }
}
