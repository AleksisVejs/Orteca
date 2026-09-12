//! Claude Code `--output-format stream-json --verbose` events.
//!
//! Tokens and cost come only from the final `result` event. Per-message usage
//! is cumulative, so summing it would count the same tokens several times.

use serde_json::Value;

use super::{CostQuality, FailureKind, ProviderEvent, Usage};

pub fn parse_line(v: &Value) -> Vec<ProviderEvent> {
    match v["type"].as_str().unwrap_or_default() {
        "system" if v["subtype"] == "init" => v["session_id"]
            .as_str()
            .map(|id| {
                vec![ProviderEvent::Started {
                    session_id: id.to_string(),
                }]
            })
            .unwrap_or_default(),
        // One message can hold text and a tool call together.
        "assistant" => v["message"]["content"]
            .as_array()
            .map(|blocks| blocks.iter().filter_map(block).collect())
            .unwrap_or_default(),
        "result" => result(v),
        _ => Vec::new(),
    }
}

fn block(b: &Value) -> Option<ProviderEvent> {
    match b["type"].as_str()? {
        "text" => Some(ProviderEvent::Text(b["text"].as_str()?.to_string())),
        "tool_use" => Some(ProviderEvent::ToolUse {
            name: b["name"].as_str()?.to_string(),
            summary: summarize(&b["input"]),
        }),
        _ => None,
    }
}

/// One line of "what did it touch" for the activity stream, not an argument
/// dump. serde_json orders keys, so `command` and `file_path` win over prose.
fn summarize(input: &Value) -> String {
    input
        .as_object()
        .into_iter()
        .flatten()
        .find_map(|(_, v)| v.as_str())
        .map(|s| s.chars().take(120).collect())
        .unwrap_or_default()
}

fn result(v: &Value) -> Vec<ProviderEvent> {
    let u = &v["usage"];
    let cost = v["total_cost_usd"].as_f64();
    let usage = Usage {
        input_tokens: n(&u["input_tokens"]),
        cached_input_tokens: n(&u["cache_read_input_tokens"]),
        output_tokens: n(&u["output_tokens"]),
        reasoning_tokens: 0,
        cost_usd: cost,
        // Claude computes this client-side from published rates. It is a good
        // guess, never a billed figure, and must never be labelled exact.
        cost_quality: match cost {
            Some(_) => CostQuality::Estimated,
            None => CostQuality::Unavailable,
        },
    };

    let text = v["result"].as_str().unwrap_or_default().to_string();
    let subtype = v["subtype"].as_str().unwrap_or_default();
    let outcome = if v["is_error"].as_bool().unwrap_or(false) || subtype != "success" {
        ProviderEvent::Failed {
            kind: match subtype {
                // Not a clock timeout, but the same thing to a user: it ran out
                // of budget before finishing.
                "error_max_turns" => FailureKind::Timeout,
                _ => FailureKind::Crashed,
            },
            message: if text.is_empty() {
                subtype.to_string()
            } else {
                text
            },
        }
    } else {
        ProviderEvent::Done {
            result: text,
            structured: v.get("structured_output").cloned(),
        }
    };

    vec![ProviderEvent::Usage(usage), outcome]
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}
