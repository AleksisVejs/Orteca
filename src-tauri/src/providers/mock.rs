//! Replays JSONL fixtures through the real parsers.
//!
//! Tests never invoke a provider CLI: neither is installed on the dev machine,
//! and a real run would spend the user's subscription. Only the *process* is
//! faked here — normalisation is the same code a live run goes through.
//!
//! Provenance: `*-run.jsonl` are written to the event shapes verified in
//! `docs/architecture.md`. `claude-auth-failure.jsonl` and
//! `codex-usage-limit.jsonl` are real captures from the argv in `run::args`,
//! recorded 2026-09-12 - both CLIs refused before doing any work, which is why
//! the happy paths are still hand-written.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{ProviderEvent, ProviderId};

/// Read `fixture` as JSONL and normalise it exactly as a live run would be.
/// Unlike `proc`, which passes a non-JSON line through as text, a malformed
/// line here is an error: a fixture is checked in, so a typo in one is a
/// broken test rather than something a provider did.
pub fn replay(id: ProviderId, fixture: &Path) -> io::Result<Vec<ProviderEvent>> {
    let input = std::fs::read_to_string(fixture)?;
    let mut events = Vec::new();
    for (index, line) in input.lines().enumerate() {
        let value = serde_json::from_str::<Value>(line).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid JSONL at line {}: {error}", index + 1),
            )
        })?;
        events.extend(id.parse_line(&value));
    }
    Ok(events)
}

/// A fixture by filename, for captures that are not a provider's happy path.
pub fn named(file: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(file)
}

/// The bundled recording for a provider.
pub fn fixture(id: ProviderId) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(format!("{}-run.jsonl", id.program()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{CostQuality, FailureKind, Usage};

    fn replay_bundled(id: ProviderId) -> Vec<ProviderEvent> {
        replay(id, &fixture(id)).expect("fixture should be readable")
    }

    fn usage(events: &[ProviderEvent]) -> Usage {
        events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::Usage(u) => Some(u.clone()),
                _ => None,
            })
            .expect("a finished run reports usage")
    }

    #[test]
    fn claude_run_normalises_to_events() {
        let events = replay_bundled(ProviderId::Claude);
        assert_eq!(
            events.first(),
            Some(&ProviderEvent::Started {
                session_id: "01J8Z9Q2K7C3N5P6R8T0V2X4Y6".into()
            })
        );
        assert!(events.contains(&ProviderEvent::ToolUse {
            name: "Read".into(),
            summary: "src-tauri/migrations/0001_init.sql".into(),
        }));
        // Keys are ordered, so the command wins over its description.
        assert!(events.contains(&ProviderEvent::ToolUse {
            name: "Bash".into(),
            summary: "git diff --stat".into(),
        }));
        assert_eq!(
            events.last(),
            Some(&ProviderEvent::Done {
                result: "The index already orders recents by opened_seq.".into(),
                structured: None,
            })
        );
    }

    #[test]
    fn claude_cost_is_estimated_never_exact() {
        let u = usage(&replay_bundled(ProviderId::Claude));
        assert_eq!(u.cost_quality, CostQuality::Estimated);
        assert_eq!(u.cost_usd, Some(0.0412));
        // Cache writes are billed as input, so they are not cached reads.
        assert_eq!(u.cached_input_tokens, 14300);
        assert_eq!(u.input_tokens, 4040);
        assert_eq!(u.output_tokens, 612);
    }

    #[test]
    fn codex_run_normalises_to_events() {
        let events = replay_bundled(ProviderId::Codex);
        assert_eq!(
            events.first(),
            Some(&ProviderEvent::Started {
                session_id: "0199c0de-4f21-7a55-9b0e-2c7d81e6a4f3".into()
            })
        );
        assert!(events.contains(&ProviderEvent::ToolUse {
            name: "Shell".into(),
            summary: "git status --porcelain".into(),
        }));
        assert!(events.contains(&ProviderEvent::ToolUse {
            name: "Edit".into(),
            summary: "src-tauri/src/store.rs".into(),
        }));
        assert!(events.contains(&ProviderEvent::Text(
            "Added the opened_seq bump to touch_project.".into()
        )));
        // Reasoning is not an assistant message and must not surface as text.
        assert!(!events.contains(&ProviderEvent::Text("Check the store before editing.".into())));
    }

    /// A real `codex exec` run, captured from the app on 2026-09-12: it edited
    /// a file and ran seven commands. `codex-run.jsonl` beside it is written to
    /// the shapes in `docs/architecture.md` and had never been checked against
    /// a live write path, which is how the item types Codex reports its own
    /// tool failures in went unnoticed until a run did nothing and said so.
    #[test]
    fn a_recorded_write_run_normalises_to_events() {
        let events = replay(ProviderId::Codex, &named("codex-write-run.jsonl"))
            .expect("a recording must load as a fixture");

        // The edit the run was asked for.
        assert!(
            events.iter().any(|e| matches!(
                e,
                ProviderEvent::ToolUse { name, summary } if name == "Edit" && summary.ends_with("slug.js")
            )),
            "the file change is missing"
        );
        // Only completed commands become events; `item.started` carries none,
        // which is why run::stream keeps those lines under `kind = unknown`.
        let shells = events
            .iter()
            .filter(|e| matches!(e, ProviderEvent::ToolUse { name, .. } if name == "Shell"))
            .count();
        assert_eq!(shells, 7, "one event per completed command, not per start");

        let u = usage(&events);
        // Codex reports cached reads inside its input count; folding them in
        // again would report this run as nine times the size it was.
        assert_eq!(u.cached_input_tokens, 133_248);
        assert_eq!(u.input_tokens, 150_095 - 133_248);
        assert_eq!(u.cost_usd, None);
        assert_eq!(u.cost_quality, CostQuality::Unavailable);
    }

    #[test]
    fn codex_reports_tokens_and_no_cost() {
        let u = usage(&replay_bundled(ProviderId::Codex));
        assert_eq!(u.cost_usd, None);
        assert_eq!(u.cost_quality, CostQuality::Unavailable);
        assert_eq!(u.reasoning_tokens, 128);
        assert_eq!(u.cached_input_tokens, 7680);
    }

    #[test]
    fn codex_documented_item_type_is_normalised() {
        let line = serde_json::json!({
            "type": "item.completed",
            "item": {"id": "item_1", "type": "agent_message", "text": "done"}
        });
        assert_eq!(
            ProviderId::Codex.parse_line(&line),
            vec![ProviderEvent::Text("done".into())]
        );
    }

    #[test]
    fn codex_top_level_error_is_a_failure() {
        let line = serde_json::json!({"type": "error", "message": "connection lost"});
        assert_eq!(
            ProviderId::Codex.parse_line(&line),
            vec![ProviderEvent::Failed {
                kind: FailureKind::Crashed,
                message: "connection lost".into(),
            }]
        );
    }

    #[test]
    fn a_failed_claude_result_is_a_failure_not_a_done() {
        let line = serde_json::json!({
            "type": "result", "subtype": "error_max_turns", "is_error": true,
            "session_id": "s", "usage": {"input_tokens": 10, "output_tokens": 2}
        });
        assert_eq!(
            ProviderId::Claude.parse_line(&line).last(),
            Some(&ProviderEvent::Failed {
                kind: FailureKind::Timeout,
                message: "error_max_turns".into(),
            })
        );
    }

    #[test]
    fn a_result_with_no_cost_field_is_unavailable_not_zero() {
        let line = serde_json::json!({
            "type": "result", "subtype": "success", "result": "done",
            "usage": {"input_tokens": 10, "output_tokens": 2}
        });
        let u = usage(&ProviderId::Claude.parse_line(&line));
        assert_eq!(u.cost_usd, None);
        assert_eq!(u.cost_quality, CostQuality::Unavailable);
    }

    #[test]
    fn claude_cache_writes_are_counted_as_input() {
        let line = serde_json::json!({
            "type": "result", "subtype": "success", "result": "done",
            "total_cost_usd": 0.1,
            "usage": {
                "input_tokens": 10,
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 30,
                "output_tokens": 2
            }
        });
        let u = usage(&ProviderId::Claude.parse_line(&line));
        assert_eq!(u.input_tokens, 30);
        assert_eq!(u.cached_input_tokens, 30);
    }

    #[test]
    fn a_zero_claude_cost_is_unknown_not_free() {
        let line = serde_json::json!({
            "type": "result", "subtype": "success", "result": "done",
            "total_cost_usd": 0.0,
            "usage": {"input_tokens": 10, "output_tokens": 2}
        });
        let u = usage(&ProviderId::Claude.parse_line(&line));
        assert_eq!(u.cost_usd, None);
        assert_eq!(u.cost_quality, CostQuality::Unavailable);
    }

    #[test]
    fn malformed_fixture_lines_fail_replay() {
        let path = std::env::temp_dir().join(format!(
            "orteca-malformed-fixture-{}.jsonl",
            std::process::id()
        ));
        std::fs::write(&path, "{not json}\n").unwrap();
        let error = replay(ProviderId::Codex, &path).unwrap_err();
        std::fs::remove_file(path).unwrap();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    /// Guards the reason `result` is the only source of usage: nobody should
    /// later "improve" this by summing assistant messages, whose output counts
    /// are placeholders repeated across every message of one API response.
    #[test]
    fn an_interrupted_claude_run_reports_no_usage_rather_than_a_wrong_one() {
        let assistant = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant",
                        "content": [{"type": "text", "text": "working"}],
                        "usage": {"input_tokens": 900, "output_tokens": 4}}
        });
        let events = ProviderId::Claude.parse_line(&assistant);
        assert_eq!(events, vec![ProviderEvent::Text("working".into())]);
        assert!(!events
            .iter()
            .any(|e| matches!(e, ProviderEvent::Usage(_))));
    }

    #[test]
    fn a_provider_error_message_picks_its_failure_kind() {
        let claude = serde_json::json!({
            "type": "result", "subtype": "error_during_execution", "is_error": true,
            "result": "Claude AI usage limit reached",
            "usage": {"input_tokens": 10, "output_tokens": 2}
        });
        assert_eq!(
            ProviderId::Claude.parse_line(&claude).last(),
            Some(&ProviderEvent::Failed {
                kind: FailureKind::UsageLimit,
                message: "Claude AI usage limit reached".into(),
            })
        );

        let codex = serde_json::json!({
            "type": "turn.failed",
            "error": {"message": "429 Too Many Requests"}
        });
        assert_eq!(
            ProviderId::Codex.parse_line(&codex),
            vec![ProviderEvent::Failed {
                kind: FailureKind::RateLimit,
                message: "429 Too Many Requests".into(),
            }]
        );
    }

    #[test]
    fn unknown_lines_are_ignored_rather_than_guessed_at() {
        let line = serde_json::json!({"type": "something.new", "payload": 1});
        assert!(ProviderId::Claude.parse_line(&line).is_empty());
        assert!(ProviderId::Codex.parse_line(&line).is_empty());
    }

    /// Real capture: `claude` installed but logged out. The trap is that the
    /// final event says `subtype: "success"` while `is_error` is true, so a
    /// parser keying on subtype alone would report a clean run that did nothing.
    #[test]
    fn a_logged_out_claude_capture_is_a_failure_not_a_success() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("claude-auth-failure.jsonl");
        let events = replay(ProviderId::Claude, &path).expect("fixture should be readable");

        assert!(matches!(events.first(), Some(ProviderEvent::Started { .. })));
        assert!(
            !events.iter().any(|e| matches!(e, ProviderEvent::Done { .. })),
            "a refused run must never report Done"
        );
        let failure = events
            .iter()
            .find_map(|e| match e {
                ProviderEvent::Failed { kind, message } => Some((kind, message)),
                _ => None,
            })
            .expect("a refused run reports a failure");
        assert_eq!(*failure.0, FailureKind::AuthExpired);
        assert!(failure.1.contains("/login"));
        // Zero tokens billed is not a zero-cost run, it is an unknown one.
        assert_eq!(usage(&events).cost_quality, CostQuality::Unavailable);
    }

    /// Real capture: `codex` logged in but out of credits. It never emits
    /// `turn.completed`, so there is no usage event at all - the run must still
    /// surface as a UsageLimit failure rather than an empty stream.
    #[test]
    fn an_out_of_credit_codex_capture_reports_a_usage_limit() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("codex-usage-limit.jsonl");
        let events = replay(ProviderId::Codex, &path).expect("fixture should be readable");

        assert!(matches!(events.first(), Some(ProviderEvent::Started { .. })));
        assert!(!events.iter().any(|e| matches!(e, ProviderEvent::Usage(_))));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderEvent::Failed { kind: FailureKind::UsageLimit, .. }
        )));
    }
}
