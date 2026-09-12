//! Provider detection and event normalisation.
//!
//! Both CLIs stream newline-delimited JSON and agree on nothing but the
//! newline. Everything downstream sees `ProviderEvent` and never a provider's
//! raw shape. `mock` replays recorded JSONL through these same parsers, so a
//! test exercises the real normalisation without spending a subscription.

pub mod claude;
pub mod codex;
pub mod mock;

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderId {
    Claude,
    Codex,
}

/// What a live detection found. "Not installed" is a state, not an error:
/// the app is expected to run on a machine with neither CLI present.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub id: ProviderId,
    pub program: &'static str,
    /// Resolved executable, or `None` when the CLI is not on PATH.
    pub path: Option<String>,
    /// Whatever `--version` printed, verbatim. `None` if it could not be run.
    pub version: Option<String>,
    pub auth: Auth,
    /// The best cost figure this provider can ever give. Codex reports tokens
    /// and no cost, so its metrics are `unavailable` before a run even starts.
    pub cost_quality: CostQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Auth {
    /// A credential file the CLI wrote at login. Orteca never reads its contents.
    Subscription,
    /// An API key is in the environment and will be billed per token.
    ApiKey,
    /// Installed, but no credential Orteca can see. The CLI will ask.
    SignedOut,
    /// Nothing installed, so nothing to say.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CostQuality {
    Exact,
    Estimated,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    /// Cache *reads* only. Cache writes are billed as ordinary input.
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_quality: CostQuality,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProviderEvent {
    Started {
        session_id: String,
    },
    Text(String),
    ToolUse {
        name: String,
        summary: String,
    },
    Usage(Usage),
    Done {
        /// The provider's own final answer. Codex does not report one, so it is
        /// empty there and the caller keeps the last `Text` instead.
        result: String,
        structured: Option<Value>,
    },
    Failed {
        kind: FailureKind,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    CliMissing,
    AuthExpired,
    UsageLimit,
    RateLimit,
    Timeout,
    Crashed,
    MalformedOutput,
    Cancelled,
}

impl ProviderId {
    pub const ALL: [ProviderId; 2] = [ProviderId::Claude, ProviderId::Codex];

    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// One raw JSONL line in, zero or more normalised events out. A single line
    /// can carry several: a Claude `result` is both usage and an outcome.
    pub fn parse_line(self, v: &Value) -> Vec<ProviderEvent> {
        match self {
            Self::Claude => claude::parse_line(v),
            Self::Codex => codex::parse_line(v),
        }
    }

    pub fn detect(self) -> Detected {
        let path = which(self.program());
        Detected {
            id: self,
            program: self.program(),
            version: path.as_deref().and_then(version_of),
            auth: if path.is_some() {
                self.auth()
            } else {
                Auth::Unknown
            },
            path: path.map(|p| p.display().to_string()),
            cost_quality: match self {
                // Claude's own total is a client-side estimate, not a bill.
                Self::Claude => CostQuality::Estimated,
                Self::Codex => CostQuality::Unavailable,
            },
        }
    }

    /// Existence check only. Orteca never reads a credential file and never
    /// asks for a password; the CLI owns auth.
    fn auth(self) -> Auth {
        let (key, credential) = match self {
            Self::Claude => ("ANTHROPIC_API_KEY", ".claude/.credentials.json"),
            Self::Codex => ("CODEX_API_KEY", ".codex/auth.json"),
        };
        if env::var_os(key).is_some_and(|v| !v.is_empty()) {
            return Auth::ApiKey;
        }
        match home().map(|h| h.join(credential)) {
            Some(p) if credential_file_exists(&p) => Auth::Subscription,
            _ => Auth::SignedOut,
        }
    }
}

fn home() -> Option<PathBuf> {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
}

fn credential_file_exists(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
}

/// Resolve a program against PATH x PATHEXT.
///
/// `CreateProcess` only ever appends `.exe`, so an npm shim — which is how
/// both CLIs install on Windows — is invisible to a bare program name. The
/// resolved path is also what the process runner will spawn in Milestone 4.
pub fn which(program: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let pathext = env::var("PATHEXT").unwrap_or_default();
    which_in(program, &path, &pathext, &env::current_dir().ok()?)
}

fn which_in(program: &str, path: &OsStr, pathext: &str, cwd: &Path) -> Option<PathBuf> {
    let pathext = if pathext.split(';').any(|ext| !ext.trim().is_empty()) {
        pathext
    } else {
        ".COM;.EXE;.BAT;.CMD"
    };
    let extensions: Vec<String> = pathext
        .split(';')
        .map(|ext| ext.trim().trim_matches('"'))
        .filter(|ext| !ext.is_empty())
        .map(|ext| {
            if ext.starts_with('.') {
                ext.to_string()
            } else {
                format!(".{ext}")
            }
        })
        .collect();

    env::split_paths(path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .find_map(|dir| {
            let dir = if dir.is_absolute() { dir } else { cwd.join(dir) };
            extensions
                .iter()
                .map(|ext| dir.join(format!("{program}{ext}")))
                .find(|candidate| candidate.is_file())
        })
}

/// ponytail: no timeout. `--version` on a hung shim would block this call;
/// give it a watchdog if that ever shows up in the wild.
fn version_of(path: &Path) -> Option<String> {
    let out = Command::new(path).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("orteca-provider-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn which_normalises_pathext_and_relative_path_entries() {
        let cwd = temp_dir("which-relative");
        let bin = cwd.join("bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::write(bin.join("agent.CMD"), "").unwrap();
        std::fs::write(bin.join("agent.EXE"), "").unwrap();

        let found = which_in("agent", std::ffi::OsStr::new("bin"), "CMD;.EXE", &cwd);
        assert_eq!(found, Some(bin.join("agent.CMD")));
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn which_uses_safe_defaults_without_current_directory_fallback() {
        let cwd = temp_dir("which-defaults");
        let bin = cwd.join("quoted bin");
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join("agent.EXE");
        std::fs::write(&executable, "").unwrap();
        let quoted = format!("\"{}\"", bin.display());

        assert_eq!(
            which_in("agent", std::ffi::OsStr::new(&quoted), "", &cwd),
            Some(executable)
        );
        std::fs::write(cwd.join("agent.EXE"), "").unwrap();
        assert_eq!(
            which_in("agent", std::ffi::OsStr::new(""), ".EXE", &cwd),
            None
        );
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn credential_detection_rejects_directories_and_empty_files() {
        let cwd = temp_dir("credentials");
        let credential = cwd.join("auth.json");
        std::fs::create_dir(&credential).unwrap();
        assert!(!credential_file_exists(&credential));
        std::fs::remove_dir(&credential).unwrap();
        std::fs::write(&credential, "").unwrap();
        assert!(!credential_file_exists(&credential));
        std::fs::write(&credential, "{}").unwrap();
        assert!(credential_file_exists(&credential));
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn version_detection_executes_a_cmd_shim() {
        let cwd = temp_dir("cmd-version");
        let shim = cwd.join("agent.cmd");
        std::fs::write(&shim, "@echo off\r\necho agent 1.2.3\r\n").unwrap();
        assert_eq!(version_of(&shim), Some("agent 1.2.3".into()));
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn which_resolves_a_pathext_extension() {
        // cmd.exe is on PATH as "cmd", never as "cmd.exe" spelled out.
        let found = which("cmd").expect("cmd should be on PATH");
        assert!(found.is_file());
        assert_eq!(
            found.extension().map(|e| e.to_ascii_lowercase()),
            Some("exe".into())
        );
    }

    #[test]
    fn a_missing_cli_is_a_state_not_an_error() {
        assert!(which("orteca-no-such-program").is_none());
    }

    #[test]
    fn detection_never_panics_on_this_machine() {
        // Neither CLI is installed on the dev machine, and that must be fine.
        for id in ProviderId::ALL {
            let d = id.detect();
            assert_eq!(d.path.is_none(), d.version.is_none() && d.auth == Auth::Unknown);
        }
    }

    #[test]
    fn codex_can_never_report_a_cost() {
        assert_eq!(
            ProviderId::Codex.detect().cost_quality,
            CostQuality::Unavailable
        );
    }
}
