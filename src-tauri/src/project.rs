//! Opening a project: git state and the trust scan.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::error::{AppError, ErrorKind, Result};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitState {
    pub is_repo: bool,
    pub root: Option<String>,
    /// Branch name, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `None` on a repo with no commits yet.
    pub head: Option<String>,
    pub dirty: bool,
    pub dirty_count: usize,
}

/// A file in the target repo that makes a provider CLI execute repo-controlled
/// configuration. Claude Code's `-p` runs these with no trust prompt of its own
/// (only `--bare` skips them, and `--bare` also disables subscription auth), so
/// Orteca has to ask instead.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustFinding {
    pub path: String,
    pub reason: &'static str,
}

const TRUST_TARGETS: &[(&str, &str)] = &[
    (".claude/settings.json", "can define hooks that run shell commands"),
    (".claude/settings.local.json", "can define hooks that run shell commands"),
    (".claude/hooks", "shell commands run around every tool call"),
    (".mcp.json", "starts MCP servers as local processes"),
    ("CLAUDE.md", "injected into the agent's instructions"),
    ("AGENTS.md", "injected into the agent's instructions"),
    (".codex/config.toml", "overrides Codex sandbox and approval settings"),
];

pub fn trust_scan(root: &Path) -> Vec<TrustFinding> {
    TRUST_TARGETS
        .iter()
        .filter(|(rel, _)| root.join(rel).exists())
        .map(|(rel, reason)| TrustFinding { path: (*rel).to_string(), reason })
        .collect()
}

pub fn git_state(dir: &Path) -> GitState {
    let Some(root) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return GitState::default();
    };

    let branch = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|b| b != "HEAD"); // detached

    let status = git(dir, &["status", "--porcelain"]).unwrap_or_default();
    let dirty_count = status.lines().filter(|l| !l.is_empty()).count();

    GitState {
        is_repo: true,
        root: Some(root),
        branch,
        head: git(dir, &["rev-parse", "HEAD"]),
        dirty: dirty_count > 0,
        dirty_count,
    }
}

/// Display name for a project directory: the folder name.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub fn validate_dir(path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path);
    if !p.is_dir() {
        return Err(AppError::new(
            ErrorKind::NotFound,
            format!("{path} is not a folder that exists"),
        ));
    }
    Ok(p)
}

/// Run git and return trimmed stdout, or `None` if git failed or isn't there.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).current_dir(dir).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("orteca-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn trust_scan_finds_executable_config_and_ignores_clean_repos() {
        let dir = temp_dir("trust");
        assert!(trust_scan(&dir).is_empty(), "a plain folder has nothing to warn about");

        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "# hi").unwrap();

        let found: Vec<_> = trust_scan(&dir).into_iter().map(|f| f.path).collect();
        assert!(found.contains(&".claude/settings.json".to_string()));
        assert!(found.contains(&"CLAUDE.md".to_string()));
        assert_eq!(found.len(), 2, "only what exists is reported");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn git_state_reports_not_a_repo_for_a_plain_folder() {
        let dir = temp_dir("nogit");
        let state = git_state(&dir);
        assert!(!state.is_repo);
        assert!(state.head.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn git_state_sees_branch_and_dirtiness() {
        let dir = temp_dir("git");
        let run = |args: &[&str]| {
            Command::new("git").args(args).current_dir(&dir).output().unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);

        // No commits yet: a repo, but no HEAD.
        assert!(git_state(&dir).is_repo);
        assert!(git_state(&dir).head.is_none());

        std::fs::write(dir.join("a.txt"), "one").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);

        let clean = git_state(&dir);
        assert_eq!(clean.branch.as_deref(), Some("main"));
        assert!(clean.head.is_some());
        assert!(!clean.dirty, "just committed, should be clean");

        std::fs::write(dir.join("a.txt"), "two").unwrap();
        let dirty = git_state(&dir);
        assert!(dirty.dirty);
        assert_eq!(dirty.dirty_count, 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
