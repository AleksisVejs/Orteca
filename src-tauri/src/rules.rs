//! The ruleset and repository profile a run's system prompt carries. Static
//! text, byte-identical between calls and free of dates, so the provider's
//! prompt cache keeps it for every turn.

use std::path::PathBuf;

use crate::intent::TaskType;

const BASE: &str = include_str!("../rulesets/base.md");

fn rules(task_type: TaskType) -> &'static str {
    match task_type {
        TaskType::Chat => include_str!("../rulesets/chat.md"),
        TaskType::Question => include_str!("../rulesets/question.md"),
        TaskType::CodeChange => include_str!("../rulesets/code_change.md"),
        TaskType::Debug => include_str!("../rulesets/debug.md"),
        TaskType::Plan => include_str!("../rulesets/plan.md"),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ruleset {
    pub task_type: TaskType,
    /// `code_change@1a2b3c4d`, the name and a hash of the rules: what the task
    /// log records, so a changed ruleset is told apart from the old one.
    pub label: String,
    pub text: String,
    /// Folders (ending in `/`) and file patterns not worth reading, which Claude is denied
    /// through its Read rules. A hint, not a fence: a shell still reads them.
    pub skip: Vec<String>,
}

impl Ruleset {
    pub fn new(task_type: TaskType, profile: &str) -> Self {
        // A Windows checkout may have turned the files' line ends into CRLF.
        let rules = format!("{BASE}{}", rules(task_type)).replace('\r', "");
        let label = format!("{}@{:08x}", task_type.name(), fnv(&rules) as u32);
        let mut text = rules;
        if !profile.trim().is_empty() {
            text.push_str("\nRepository profile:\n");
            text.push_str(profile.trim());
            text.push('\n');
        }
        Ruleset { task_type, label, text, skip: Vec::new() }
    }

    /// `claude --settings`: Read rules for `skip`, which also cover Grep and Glob.
    pub fn claude_settings(&self) -> Option<String> {
        let deny: Vec<String> = self
            .skip
            .iter()
            .map(|p| if p.ends_with('/') { format!("Read(./{p}**)") } else { format!("Read(**/{p})") })
            .collect();
        (!deny.is_empty()).then(|| serde_json::json!({ "permissions": { "deny": deny } }).to_string())
    }

    /// Written once where the CLI can read it and never in the repository,
    /// named by its content so a file a running call reads is never rewritten.
    pub fn file(&self) -> Option<PathBuf> {
        let dir = crate::proc::owned_temp().unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("orteca-rules-{:016x}.md", fnv(&self.text)));
        if !path.exists() {
            std::fs::write(&path, &self.text).ok()?;
        }
        Some(path)
    }

    /// Codex's `developer_instructions`, which adds to its own prompt rather
    /// than replacing it. One line: it is a TOML string passed through a shim.
    pub fn codex_arg(&self) -> String {
        let escaped = self.text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
        format!("developer_instructions=\"{escaped}\"")
    }
}

/// FNV-1a: stable across Rust versions, unlike `DefaultHasher`.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ruleset_is_stable_and_codex_gets_it_on_one_line() {
        let a = Ruleset::new(TaskType::Debug, "- Test: \"cargo test\"\n- Do not read: target/");
        assert_eq!(a, Ruleset::new(TaskType::Debug, "- Test: \"cargo test\"\n- Do not read: target/"));
        assert!(a.label.starts_with("debug@"));
        assert_eq!(a.label, Ruleset::new(TaskType::Debug, "").label, "the profile is not the ruleset's version");
        let arg = a.codex_arg();
        assert!(!arg.contains('\n') && arg.contains(r#"\"cargo test\""#), "{arg}");
        let mut b = a.clone();
        b.skip = vec!["node_modules/".into(), "*.lock".into(), "package-lock.json".into()];
        assert_eq!(
            b.claude_settings().unwrap(),
            r#"{"permissions":{"deny":["Read(./node_modules/**)","Read(**/*.lock)","Read(**/package-lock.json)"]}}"#
        );
        assert_eq!(a.claude_settings(), None);
        for t in TaskType::ALL {
            assert!(!Ruleset::new(t, "").text.contains('%'), "cmd.exe would expand it");
        }
    }
}
