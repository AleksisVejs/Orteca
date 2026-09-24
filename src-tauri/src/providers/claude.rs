//! Claude Code `--output-format stream-json --verbose` events.
//!
//! Tokens and cost come only from the final `result` event. Per-message usage
//! is cumulative, so summing it would count the same tokens several times.

use serde_json::Value;

use super::{classify_failure, CostQuality, FailureKind, FileEdit, ProviderEvent, Usage};

/// One user message in the shape `--input-format stream-json` accepts.
///
/// This shape was verified against the CLI, not taken from the spec: the
/// architecture doc guessed `{"type":"user","text":"..."}`, which the CLI does
/// not accept. A live run confirmed this one by echoing it back under
/// `--replay-user-messages`. Serialised through serde so a prompt containing
/// quotes, newlines or backslashes cannot break the line framing.
pub fn user_message(text: &str) -> String {
    serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": text },
    })
    .to_string()
}

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
        "system" if v["subtype"] == "api_retry" => api_retry(v).into_iter().collect(),
        "result" => result(v),
        "user" => edit_result(v).into_iter().collect(),
        // Sent before the answer on every call; `allowed` and `allowed_warning`
        // are recorded. `rejected` is the plan's limit being used up. The result
        // that follows may word it in a way `classify_failure` does not know,
        // so the kind is set here. Not yet seen in a recording.
        "rate_limit_event"
            if v["rate_limit_info"]["status"] == "rejected"
                && v["rate_limit_info"]["isUsingOverage"] != true =>
        {
            let window = match v["rate_limit_info"]["rateLimitType"].as_str() {
                Some("five_hour") => "session",
                Some("seven_day") => "weekly",
                _ => "plan",
            };
            vec![ProviderEvent::Failed {
                kind: FailureKind::UsageLimit,
                message: format!("Claude's {window} usage limit is used up."),
            }]
        }
        _ => Vec::new(),
    }
}

/// The CLI retries a failed API call by itself. A retry that cannot succeed -
/// the account, not the request, is the problem - fails the run now instead
/// of after every attempt. A short rate-limit or overload wait is left alone.
/// Shape read from claude 2.1.280's bundle; not yet seen in a recording.
fn api_retry(v: &Value) -> Option<ProviderEvent> {
    let error = v["error"].as_str()?;
    let (kind, message) = match error {
        "billing_error" => (FailureKind::UsageLimit, "Claude reports a billing problem or a used-up plan."),
        "authentication_failed" | "oauth_org_not_allowed" | "account_on_hold" | "verification_required" => {
            (FailureKind::AuthExpired, "Claude could not sign in to its account. Sign in again with `claude auth login`.")
        }
        "rate_limit" if v["retry_delay_ms"].as_u64().is_some_and(|ms| ms >= 60_000) => {
            (FailureKind::RateLimit, "Claude is rate limited for more than a minute.")
        }
        _ => return None,
    };
    Some(ProviderEvent::Failed { kind, message: message.into() })
}

/// Claude returns a schema-constrained value in the final result event.
pub fn structured_output(v: &Value) -> Option<Value> {
    (v["type"] == "result")
        .then(|| v.get("structured_output").cloned())
        .flatten()
}

fn block(b: &Value) -> Option<ProviderEvent> {
    match b["type"].as_str()? {
        "text" => Some(ProviderEvent::Text(b["text"].as_str()?.to_string())),
        // A redacted block has only a signature; nothing to show.
        "thinking" => Some(b["thinking"].as_str()?.trim())
            .filter(|t| !t.is_empty())
            .map(|t| ProviderEvent::Thinking(t.to_string())),
        "tool_use" => {
            let name = b["name"].as_str()?;
            let editing = matches!(name, "Edit" | "Write" | "MultiEdit");
            Some(ProviderEvent::ToolUse {
                name: name.to_string(),
                summary: summarize(&b["input"]),
                id: if editing {
                    b["id"].as_str().map(str::to_string)
                } else {
                    None
                },
                changes: if editing {
                    b["input"]["file_path"]
                        .as_str()
                        .map(|path| FileEdit {
                            path: path.to_string(),
                            patch: None,
                        })
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
                },
            })
        }
        _ => None,
    }
}

fn edit_result(v: &Value) -> Option<ProviderEvent> {
    let result = v["message"]["content"]
        .as_array()?
        .iter()
        .find(|block| block["type"] == "tool_result")?;
    let data = &v["tool_use_result"];
    let failed = result["is_error"] == true;
    if !failed && data["filePath"].as_str().is_none() {
        return None;
    }
    let changes = data["filePath"]
        .as_str()
        .map(|path| FileEdit {
            path: path.to_string(),
            patch: if failed { None } else { edit_patch(data) },
        })
        .into_iter()
        .collect();
    Some(ProviderEvent::ToolResult {
        id: result["tool_use_id"].as_str()?.to_string(),
        changes,
        failed,
    })
}

fn edit_patch(data: &Value) -> Option<String> {
    let mut patch = String::new();
    for hunk in data["structuredPatch"].as_array()? {
        patch.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk["oldStart"].as_u64()?,
            hunk["oldLines"].as_u64()?,
            hunk["newStart"].as_u64()?,
            hunk["newLines"].as_u64()?
        ));
        for line in hunk["lines"].as_array()? {
            patch.push_str(line.as_str()?);
            patch.push('\n');
        }
    }
    Some(patch)
}

/// The input fields that say what a tool call is about, best first.
///
/// This used to be left to key ordering: `serde_json` builds a `BTreeMap`, so
/// iteration is alphabetical, and `command` and `file_path` do sort ahead of
/// the prose fields beside them. But Write's input is `{content, file_path}`,
/// where `content` sorts first - so every file an agent wrote showed 120
/// characters of the file's own body instead of its name. Name the fields
/// rather than hoping at their spelling.
const IDENTIFYING: &[&str] = &[
    "file_path",
    "command",
    "pattern",
    "path",
    "url",
    "query",
    "description",
    "prompt",
];

/// One line of "what did it touch" for the activity stream, not an argument dump.
fn summarize(input: &Value) -> String {
    let Some(object) = input.as_object() else {
        return String::new();
    };
    IDENTIFYING
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        // A tool nobody listed still says something rather than nothing.
        .or_else(|| object.values().find_map(Value::as_str))
        .unwrap_or_default()
        .chars()
        .take(120)
        .collect()
}

/// Only the final `result` carries trustworthy numbers. Per-message `usage`
/// reports the output count the API had at `message_start`, and one API
/// response can produce several assistant messages all repeating that same
/// placeholder - so summing them, or keeping the last, both understate the
/// run. A run that dies before `result` therefore has no honest token count,
/// and must be recorded as unavailable rather than as a zero.
fn result(v: &Value) -> Vec<ProviderEvent> {
    let u = &v["usage"];
    let cost = v["total_cost_usd"].as_f64().filter(|cost| *cost > 0.0);
    let usage = Usage {
        // `result` has no `model` field; `modelUsage` is keyed by model id and
        // can hold a helper model too, so name the one that wrote the most.
        model: v["modelUsage"].as_object().and_then(|models| {
            models
                .iter()
                .max_by_key(|(_, m)| n(&m["outputTokens"]))
                .map(|(id, _)| id.clone())
        }),
        input_tokens: n(&u["input_tokens"]).saturating_add(n(&u["cache_creation_input_tokens"])),
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
                // The CLI hit the `--max-turns` ceiling Orteca gave it. That is
                // a budget Orteca set being reached, not a fault of the run, so
                // it is reported as itself and `run` turns it into the
                // budget-reached outcome with the work so far intact.
                "error_max_turns" => FailureKind::BudgetReached,
                _ => classify_failure(&text),
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
            turns: n(&v["num_turns"]).max(1) as u32,
        }
    };

    vec![ProviderEvent::Usage(usage), outcome]
}

fn n(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_edits_keep_the_completed_patch_and_call_id() {
        let events: Vec<_> = include_str!("../../fixtures/claude-isolated-run.jsonl")
            .lines()
            .flat_map(|line| parse_line(&serde_json::from_str(line).unwrap()))
            .collect();
        let (id, changes) = events
            .iter()
            .find_map(|event| match event {
                ProviderEvent::ToolUse {
                    name, id, changes, ..
                } if name == "Edit" => Some((id, changes)),
                _ => None,
            })
            .unwrap();
        assert!(changes[0].path.ends_with("notes.txt"));
        assert_eq!(changes[0].patch, None, "a request is not a completed edit");
        let completed = events.iter().find(|event| matches!(event,
            ProviderEvent::ToolResult { id: result_id, .. } if Some(result_id) == id.as_ref()
        )).unwrap();
        let ProviderEvent::ToolResult {
            changes, failed, ..
        } = completed
        else {
            unreachable!()
        };
        assert!(!failed);
        assert_eq!(
            changes[0].patch.as_deref(),
            Some("@@ -1,1 +1,1 @@\n-helo world\n+hello world\n")
        );
        // This is the exact shape both SQLite and the live stream receive.
        let saved = serde_json::to_string(completed).unwrap();
        assert_eq!(
            serde_json::from_str::<ProviderEvent>(&saved).unwrap(),
            *completed
        );
        assert!(saved.contains("toolResult"));
    }

    #[test]
    fn failed_edits_and_old_events_never_invent_a_patch() {
        let events = parse_line(&serde_json::json!({"type":"user", "message":{"content":[{
            "type":"tool_result", "tool_use_id":"bad-edit", "is_error":true, "content":"not found"
        }]}}));
        assert!(
            matches!(&events[0], ProviderEvent::ToolResult { failed: true, changes, .. } if changes.is_empty())
        );
        let old: ProviderEvent =
            serde_json::from_str(r#"{"kind":"toolUse","data":{"name":"Edit","summary":"a.ts"}}"#)
                .unwrap();
        assert!(
            matches!(old, ProviderEvent::ToolUse { id: None, changes, .. } if changes.is_empty())
        );
        assert_eq!(
            edit_patch(&serde_json::json!({"structuredPatch":[{"oldStart":1}]})),
            None
        );
    }

    #[test]
    fn thinking_is_kept_unless_redacted() {
        assert_eq!(
            super::block(&serde_json::json!({"type":"thinking", "thinking":"Check the router first.", "signature":"x"})),
            Some(ProviderEvent::Thinking("Check the router first.".into()))
        );
        assert_eq!(super::block(&serde_json::json!({"type":"thinking", "thinking":"", "signature":"x"})), None);
    }

    fn summary(input: Value) -> String {
        let block = serde_json::json!({"type": "tool_use", "name": "T", "input": input});
        match super::block(&block) {
            Some(ProviderEvent::ToolUse { summary, .. }) => summary,
            other => panic!("{other:?}"),
        }
    }

    /// The bug: a Write call is `{content, file_path}`, and taking whichever
    /// string came first meant the activity stream printed the contents of
    /// every file an agent wrote instead of its name.
    #[test]
    fn a_tool_call_is_summarised_by_what_it_touched_not_by_what_it_carried() {
        assert_eq!(
            summary(serde_json::json!({"content": "fn main() {}", "file_path": "src/main.rs"})),
            "src/main.rs",
            "a Write must name the file, not quote it"
        );
        assert_eq!(
            summary(serde_json::json!({"command": "cargo test", "description": "run tests"})),
            "cargo test"
        );
        assert_eq!(
            summary(serde_json::json!({"file_path": "a.rs", "old_string": "x", "new_string": "y"})),
            "a.rs"
        );
        assert_eq!(
            summary(serde_json::json!({"pattern": "TODO", "path": "src"})),
            "TODO"
        );
        // Nothing recognised: still better than an empty line.
        assert_eq!(
            summary(serde_json::json!({"whatever": "something"})),
            "something"
        );
        assert_eq!(summary(serde_json::json!({"count": 3})), "");
    }

    /// Cut by characters, never by bytes: a multi-byte path would panic.
    #[test]
    fn a_long_summary_is_cut_without_splitting_a_character() {
        let long = "é".repeat(200);
        assert_eq!(
            summary(serde_json::json!({"file_path": long}))
                .chars()
                .count(),
            120
        );
    }

    /// Recorded 2.1.269 shape: no `model` on `result`, and `modelUsage` may
    /// list a helper model beside the one that did the work.
    #[test]
    fn the_model_is_the_one_that_wrote_the_most() {
        let events = parse_line(
            &serde_json::json!({"type":"result", "subtype":"success", "result":"ok",
            "usage": {"input_tokens": 2, "output_tokens": 640},
            "modelUsage": {
                "claude-haiku-4-5": {"outputTokens": 30},
                "claude-sonnet-5": {"outputTokens": 610}
            }}),
        );
        let usage = events.iter().find_map(|e| match e {
            ProviderEvent::Usage(u) => Some(u),
            _ => None,
        });
        assert_eq!(
            usage.and_then(|u| u.model.as_deref()),
            Some("claude-sonnet-5")
        );
    }

    #[test]
    fn a_rejected_rate_limit_is_a_usage_limit_and_a_warning_is_not() {
        let rejected = parse_line(&serde_json::json!({"type": "rate_limit_event",
            "rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour", "isUsingOverage": false}}));
        assert!(
            matches!(rejected.as_slice(), [ProviderEvent::Failed { kind: FailureKind::UsageLimit, message }] if message.contains("session")),
            "{rejected:?}"
        );
        let warned = parse_line(&serde_json::json!({"type": "rate_limit_event",
            "rate_limit_info": {"status": "allowed_warning", "rateLimitType": "seven_day"}}));
        assert!(warned.is_empty(), "a warning is not a stop");
        assert_eq!(
            classify_failure("You've hit your session limit · resets 4pm"),
            FailureKind::UsageLimit
        );
    }

    #[test]
    fn an_api_retry_fails_only_when_retrying_cannot_help() {
        let retry = |error: &str, ms: u64| parse_line(&serde_json::json!({"type": "system", "subtype": "api_retry",
            "attempt": 1, "max_retries": 10, "retry_delay_ms": ms, "error_status": 429, "error": error}));
        assert!(matches!(retry("billing_error", 500)[..], [ProviderEvent::Failed { kind: FailureKind::UsageLimit, .. }]));
        assert!(matches!(retry("authentication_failed", 500)[..], [ProviderEvent::Failed { kind: FailureKind::AuthExpired, .. }]));
        assert!(matches!(retry("rate_limit", 120_000)[..], [ProviderEvent::Failed { kind: FailureKind::RateLimit, .. }]));
        assert!(retry("rate_limit", 2_000).is_empty());
        assert!(retry("overloaded", 2_000).is_empty());
    }
}
