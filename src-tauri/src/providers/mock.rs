//! Replays JSONL fixtures through the real parsers.
//!
//! Tests never invoke a provider CLI: neither is installed on the dev machine,
//! and a real run would spend the user's subscription. Only the *process* is
//! faked here — normalisation is the same code a live run goes through.
//!
//! Provenance: `fixtures/*.jsonl` are written to the event shapes verified in
//! `docs/architecture.md`, not captured from a live session. Replace a file
//! with a real capture the first time one is available; nothing else changes.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{ProviderEvent, ProviderId};

/// Read `fixture` as JSONL and normalise it exactly as a live run would be.
/// Lines that are not JSON are dropped, the same way `proc` hands them off.
pub fn replay(id: ProviderId, fixture: &Path) -> io::Result<Vec<ProviderEvent>> {
    Ok(std::fs::read_to_string(fixture)?
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .flat_map(|v| id.parse_line(&v))
        .collect())
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
        assert_eq!(u.input_tokens, 1840);
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

    #[test]
    fn codex_reports_tokens_and_no_cost() {
        let u = usage(&replay_bundled(ProviderId::Codex));
        assert_eq!(u.cost_usd, None);
        assert_eq!(u.cost_quality, CostQuality::Unavailable);
        assert_eq!(u.reasoning_tokens, 1024);
        assert_eq!(u.cached_input_tokens, 7680);
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
    fn unknown_lines_are_ignored_rather_than_guessed_at() {
        let line = serde_json::json!({"type": "something.new", "payload": 1});
        assert!(ProviderId::Claude.parse_line(&line).is_empty());
        assert!(ProviderId::Codex.parse_line(&line).is_empty());
    }
}
