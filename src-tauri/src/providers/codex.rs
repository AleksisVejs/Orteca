//! Codex `exec --json` events.
//!
//! Codex reports tokens and no cost at all, so the parser leaves every usage
//! `unavailable`. `run` prices it afterwards from the published API rates
//! fetched here, and labels the result `estimated`: a ChatGPT sign-in is not
//! billed per token, so it is what the same tokens would cost on the API.

use serde_json::Value;

use super::{classify_failure, CostQuality, ProviderEvent, Usage};
use crate::store::Price;

/// Community-kept, no key, and its OpenAI rates matched LiteLLM's on 2026-09-14
/// where OpenRouter's did not (Sol).
const PRICES_URL: &str = "https://models.dev/api.json";

/// Download the price list with Windows' own `curl.exe`. `None` on any failure;
/// the caller keeps the list it already has.
pub fn fetch_prices() -> Option<Vec<(String, Price)>> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Absolute, so a `curl.exe` earlier on PATH is never the one that runs.
    let curl = std::path::Path::new(&std::env::var_os("SystemRoot")?).join(r"System32\curl.exe");
    let out = std::process::Command::new(curl)
        .args(["-sSfL", "--max-time", "30", PRICES_URL])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_prices(&serde_json::from_slice(&out.stdout).ok()?))
}

/// models.dev's OpenAI models. The file comes off the internet, so a model
/// missing a rate, or with a negative or non-finite one, is left unpriced.
pub fn parse_prices(v: &Value) -> Vec<(String, Price)> {
    let Some(models) = v["openai"]["models"].as_object() else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|(slug, m)| {
            let rate = |key: &str| m["cost"][key].as_f64().filter(|r| r.is_finite() && *r >= 0.0);
            Some((
                slug.clone(),
                Price {
                    input: rate("input")?,
                    output: rate("output")?,
                    cache_read: rate("cache_read")?,
                },
            ))
        })
        .collect()
}

/// USD for one usage at base rates. Reasoning is already inside output.
// ponytail: ignores the >272k-prompt tier, since usage is summed over turns and
// no one turn's prompt size survives; Orteca's turns measured 12.6k (§4.3.3).
pub fn estimate(u: &Usage, p: Price) -> f64 {
    (u.input_tokens as f64 * p.input
        + u.cached_input_tokens as f64 * p.cache_read
        + u.output_tokens as f64 * p.output)
        / 1_000_000.0
}

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
                    input_tokens: n(&u["input_tokens"])
                        .saturating_sub(n(&u["cached_input_tokens"])),
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
        let ProviderEvent::Usage(u) = &events[0] else {
            panic!("missing usage")
        };
        assert_eq!(
            u.input_tokens + u.cached_input_tokens + u.output_tokens,
            120
        );
        assert_eq!(u.model, None, "codex names no model, so none is invented");
    }

    #[test]
    fn prices_off_the_internet_are_checked_then_kept() {
        let prices = parse_prices(&serde_json::json!({"openai": {"models": {
            "gpt-5.6-luna": {"cost": {"input": 0.2, "output": 1.2, "cache_read": 0.02}},
            "no-cache-rate": {"cost": {"input": 1, "output": 2}},
            "negative": {"cost": {"input": -1, "output": 2, "cache_read": 0.1}},
            "free-text": {"cost": {"input": "cheap", "output": 2, "cache_read": 0.1}}
        }}}));
        assert_eq!(prices.len(), 1, "{prices:?}");

        let store = crate::store::Store::in_memory().unwrap();
        store.save_prices(&prices).unwrap();
        store.save_prices(&[]).unwrap();
        let luna = store.price("gpt-5.6-luna").expect("an empty fetch kept the list");
        assert_eq!(store.price("negative"), None);

        let usage = Usage {
            model: None,
            input_tokens: 1_000_000,
            cached_input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            reasoning_tokens: 500_000,
            cost_usd: None,
            cost_quality: CostQuality::Unavailable,
        };
        // Reasoning is inside output, so it is not charged twice.
        assert!((estimate(&usage, luna) - 1.42).abs() < 1e-9);
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
