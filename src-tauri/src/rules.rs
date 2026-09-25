//! The ruleset and repository profile a run's system prompt carries. Static
//! text, byte-identical between calls and free of dates, so the provider's
//! prompt cache keeps it for every turn.

use std::path::PathBuf;

use crate::intent::TaskType;

const BASE: &str = include_str!("../rulesets/base.md");
/// What an agent needs from the CLI's own system prompt, which this replaces.
const AGENT: &str = include_str!("../rulesets/agent.md");

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
    /// The text replaces the CLI's own system prompt instead of adding to it:
    /// ~6k fewer tokens a turn for Claude, ~3.5k a request for Codex.
    /// `NAV_CLIPROMPT=1` keeps the CLI's prompt, for the benchmark's before arm.
    pub replaces: bool,
}

impl Ruleset {
    pub fn new(task_type: TaskType, profile: &str) -> Self {
        let replaces = std::env::var("NAV_CLIPROMPT").is_err();
        // Talk has no tools, so it needs none of the agent guidance.
        let agent = if replaces && task_type != TaskType::Chat { AGENT } else { "" };
        // A Windows checkout may have turned the files' line ends into CRLF.
        let rules = format!("{agent}{BASE}{}", rules(task_type)).replace('\r', "");
        let label = format!("{}@{:08x}", task_type.name(), fnv(&rules) as u32);
        let mut text = rules;
        if !profile.trim().is_empty() {
            text.push_str("\nRepository profile:\n");
            text.push_str(profile.trim());
            text.push('\n');
        }
        Ruleset { task_type, label, text, skip: Vec::new(), replaces }
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

    /// Codex's `model_instructions_file`, which replaces its own base
    /// instructions, or `developer_instructions`, which adds to them. Either is
    /// one line: a TOML string passed through a shim.
    pub fn codex_arg(&self) -> String {
        if let Some(file) = self.file().filter(|_| self.replaces) {
            return format!("model_instructions_file=\"{}\"", file.display().to_string().replace('\\', "\\\\"));
        }
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
        let file = arg.strip_prefix("model_instructions_file=\"").unwrap().trim_end_matches('"').replace("\\\\", "\\");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("You are a coding agent") && text.contains("\"cargo test\""), "{text}");
        assert!(!Ruleset::new(TaskType::Chat, "").text.contains("coding agent"), "talk has no tools to explain");
        let added = Ruleset { replaces: false, ..a.clone() }.codex_arg();
        assert!(!added.contains('\n') && added.contains(r#"\"cargo test\""#), "{added}");
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
