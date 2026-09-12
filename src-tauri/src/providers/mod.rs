//! Provider detection and event normalisation.
//!
//! Both CLIs stream newline-delimited JSON and agree on nothing but the
//! newline. Everything downstream sees `ProviderEvent` and never a provider's
//! raw shape. `mock` replays recorded JSONL through these same parsers, so a
//! test exercises the real normalisation without spending a subscription.

pub mod claude;
pub mod codex;
#[cfg(test)]
pub mod mock;

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CostQuality {
    Exact,
    Estimated,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Uncached input, including cache writes. Cache reads are disjoint.
    pub input_tokens: u64,
    /// Cache *reads* only. Cache writes are billed as ordinary input.
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
    /// Already included in output_tokens; never add this again to a total.
    pub reasoning_tokens: u64,
    pub cost_usd: Option<f64>,
    pub cost_quality: CostQuality,
}

/// Adjacently tagged so every variant survives, including the newtype ones:
/// `{"kind":"text","data":"..."}`. This is both the payload the UI receives
/// and the row written to `task_events`, so there is one shape, not two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "camelCase", rename_all_fields = "camelCase")]
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

/// Only the kinds a parser actually produces. `CliMissing`, `Cancelled` and
/// `MalformedOutput` were dropped: they are the process runner's to report,
/// and it does not exist until Milestone 4. Whichever of them that runner
/// truly emits comes back then, one at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    AuthExpired,
    UsageLimit,
    RateLimit,
    Timeout,
    Crashed,
}

impl ProviderEvent {
    /// The `task_events.kind` column. Same spelling as the serialised tag.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Text(_) => "text",
            Self::ToolUse { .. } => "toolUse",
            Self::Usage(_) => "usage",
            Self::Done { .. } => "done",
            Self::Failed { .. } => "failed",
        }
    }
}

/// Neither CLI reports a machine-readable error code, so free text is the only
/// signal there is. Match on the phrases both providers share, and leave
/// anything unrecognised as `Crashed` rather than guessing it into a
/// friendlier bucket - a wrong bucket sends the user to fix the wrong thing.
pub fn classify_failure(message: &str) -> FailureKind {
    let m = message.to_ascii_lowercase();
    // "rate limit" first: a usage-limit message often mentions both.
    if m.contains("rate limit") || m.contains("too many requests") {
        FailureKind::RateLimit
    } else if m.contains("usage limit") || m.contains("quota") || m.contains("credit balance") {
        FailureKind::UsageLimit
    } else if m.contains("oauth")
        || m.contains("api key")
        || m.contains("unauthorized")
        || m.contains("authentication")
        || m.contains("/login")
    {
        FailureKind::AuthExpired
    } else {
        FailureKind::Crashed
    }
}

impl ProviderId {
    pub const ALL: [ProviderId; 2] = [ProviderId::Claude, ProviderId::Codex];

    pub fn program(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The npm package that installs this CLI. Both ship as npm globals on
    /// Windows - which is the same reason `which` has to resolve `.cmd` shims.
    /// Orteca never downloads a binary itself; it runs the package manager the
    /// user already has.
    pub fn package(self) -> &'static str {
        match self {
            Self::Claude => "@anthropic-ai/claude-code",
            Self::Codex => "@openai/codex",
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
            auth: match path.as_deref() {
                Some(p) => self.auth(p),
                None => Auth::Unknown,
            },
            path: path.map(|p| p.display().to_string()),
            cost_quality: match self {
                // Claude's own total is a client-side estimate, not a bill.
                Self::Claude => CostQuality::Estimated,
                Self::Codex => CostQuality::Unavailable,
            },
        }
    }

    /// The arguments that make a CLI report its own auth state.
    pub fn auth_args(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["auth", "status", "--json"],
            Self::Codex => &["login", "status"],
        }
    }

    /// The arguments that start an interactive sign-in. `--claudeai` is already
    /// the default, but naming it skips the "which account type" prompt - and
    /// Orteca spawns this with stdin closed, so a prompt would deadlock.
    pub fn login_args(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["auth", "login", "--claudeai"],
            Self::Codex => &["login"],
        }
    }

    /// Ask the CLI. A credential file existing proves nothing: on Windows
    /// `~/.claude/.credentials.json` holds MCP server tokens and is present for
    /// a user who has never signed in, so the old existence check reported a
    /// saved login right up until the run failed. Orteca still never reads a
    /// credential - it reads the CLI's own answer about one.
    fn auth(self, path: &Path) -> Auth {
        let Some((lines, code)) = capture(path, self.auth_args()) else {
            return Auth::Unknown;
        };
        let text = lines.join(" ");
        match self {
            // `--json` is the documented default, but parse defensively: an
            // unreadable answer is Unknown, never an optimistic "signed in".
            Self::Claude => match serde_json::from_str::<Value>(&text) {
                Ok(v) => match v["loggedIn"].as_bool() {
                    Some(false) => Auth::SignedOut,
                    Some(true) if v["authMethod"].as_str().is_some_and(is_key) => Auth::ApiKey,
                    Some(true) => Auth::Subscription,
                    None => Auth::Unknown,
                },
                Err(_) => Auth::Unknown,
            },
            Self::Codex => {
                let lower = text.to_ascii_lowercase();
                if lower.contains("not logged in") || code != Some(0) {
                    Auth::SignedOut
                } else if is_key(&lower) {
                    Auth::ApiKey
                } else if lower.contains("logged in") {
                    Auth::Subscription
                } else {
                    Auth::Unknown
                }
            }
        }
    }
}

fn is_key(s: &str) -> bool {
    let s = s.to_ascii_lowercase();
    s.contains("apikey") || s.contains("api key") || s.contains("api-key")
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

/// Run a CLI to completion and collect its non-empty stdout lines with its exit
/// code. Blocking with a deadline on purpose: detection runs before any UI is
/// drawn, and a CLI that hangs must not hang the launch screen. `None` means it
/// could not be asked at all - never a fabricated answer.
fn capture(path: &Path, args: &[&str]) -> Option<(Vec<String>, Option<i32>)> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            // Never in the user's project: detection runs before anyone has
            // consented to that repository, same reason npm install uses temp.
            let mut run = crate::proc::spawn(&path.to_string_lossy(), args, &env::temp_dir()).ok()?;
            run.close_stdin();
            let mut lines = Vec::new();
            while let Some(line) = run.lines.recv().await {
                match line {
                    crate::proc::Line::Exit(code) => return Some((lines, code)),
                    crate::proc::Line::Text(text) if !text.trim().is_empty() => {
                        lines.push(text.trim().to_string())
                    }
                    // `--json` output arrives already parsed; put it back.
                    crate::proc::Line::Json(value) => lines.push(value.to_string()),
                    _ => {}
                }
            }
            None
        }).await.ok().flatten()
    })
}

fn version_of(path: &Path) -> Option<String> {
    let (lines, code) = capture(path, &["--version"])?;
    (code == Some(0)).then(|| lines.into_iter().next()).flatten()
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

    /// `detect_providers` runs detection inside the blocking pool, and
    /// `version_of` builds its own runtime to get a timeout. A nested runtime
    /// on a thread that already has one panics, and only a real call catches it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_version_is_read_from_inside_the_blocking_pool() {
        let dir = temp_dir("blocking-pool");
        let shim = dir.join("shimmy.cmd");
        std::fs::write(&shim, "@echo off
echo 1.2.3
").unwrap();
        let found = tokio::task::spawn_blocking(move || version_of(&shim)).await.unwrap();
        assert_eq!(found.as_deref(), Some("1.2.3"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failures_are_classified_only_when_the_text_says_so() {
        assert_eq!(
            classify_failure("Claude AI usage limit reached|1751200000"),
            FailureKind::UsageLimit
        );
        assert_eq!(
            classify_failure("API Error: 429 rate limit exceeded"),
            FailureKind::RateLimit
        );
        assert_eq!(
            classify_failure("Your credit balance is too low"),
            FailureKind::UsageLimit
        );
        assert_eq!(
            classify_failure("OAuth token has expired, please run /login"),
            FailureKind::AuthExpired
        );
        // An unrecognised message must not be dressed up as something actionable.
        assert_eq!(
            classify_failure("the model produced an unexpected response"),
            FailureKind::Crashed
        );
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

    /// A shim standing in for a CLI, so auth detection is tested without ever
    /// invoking a real provider.
    fn shim(label: &str, body: &str) -> PathBuf {
        let cwd = temp_dir(label);
        let path = cwd.join("agent.cmd");
        std::fs::write(&path, format!("@echo off
{body}
")).unwrap();
        path
    }

    #[test]
    fn claude_auth_comes_from_the_cli_not_a_credential_file() {
        // The exact shape `claude auth status --json` returned when logged out.
        let out = shim("claude-out", r#"echo {"loggedIn":false,"authMethod":"none"}"#);
        assert_eq!(ProviderId::Claude.auth(&out), Auth::SignedOut);

        let sub = shim("claude-sub", r#"echo {"loggedIn":true,"authMethod":"claudeai"}"#);
        assert_eq!(ProviderId::Claude.auth(&sub), Auth::Subscription);

        let key = shim("claude-key", r#"echo {"loggedIn":true,"authMethod":"apiKey"}"#);
        assert_eq!(ProviderId::Claude.auth(&key), Auth::ApiKey);
    }

    /// `--json` pretty-prints across several lines, so the answer has to be
    /// reassembled before it parses. A one-line fixture would not catch this.
    #[test]
    fn a_pretty_printed_auth_answer_is_reassembled() {
        let pretty = shim(
            "claude-pretty",
            "echo {
echo   \"loggedIn\": false,
echo   \"authMethod\": \"none\"
echo }",
        );
        assert_eq!(ProviderId::Claude.auth(&pretty), Auth::SignedOut);
    }

    #[test]
    fn codex_auth_reads_the_status_line() {
        let out = shim("codex-out", "echo Not logged in");
        assert_eq!(ProviderId::Codex.auth(&out), Auth::SignedOut);

        // The exact line `codex login status` returned while logged in.
        let sub = shim("codex-sub", "echo Logged in using ChatGPT");
        assert_eq!(ProviderId::Codex.auth(&sub), Auth::Subscription);
    }

    /// The failure that matters: an unreadable or non-zero answer must never
    /// be optimistic. Reporting a saved login the user does not have is how the
    /// old file-existence check sent people into a run that could only fail.
    #[test]
    fn an_unreadable_auth_answer_is_never_reported_as_signed_in() {
        let garbage = shim("claude-garbage", "echo not json at all");
        assert_eq!(ProviderId::Claude.auth(&garbage), Auth::Unknown);

        let broken = shim("codex-broken", "exit /b 1");
        assert_eq!(ProviderId::Codex.auth(&broken), Auth::SignedOut);
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
    fn version_detection_has_a_deadline() {
        let cwd = temp_dir("version-deadline");
        let shim = cwd.join("agent.cmd");
        std::fs::write(&shim, "@echo off\r\nping -n 6 127.0.0.1 >nul\r\necho too late\r\n").unwrap();
        assert_eq!(version_of(&shim), None);
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
    fn missing_executable_has_no_version() {
        assert_eq!(version_of(Path::new("orteca-no-such-program")), None);
    }

    #[test]
    fn each_provider_installs_from_its_own_published_package() {
        assert_eq!(ProviderId::Claude.package(), "@anthropic-ai/claude-code");
        assert_eq!(ProviderId::Codex.package(), "@openai/codex");
    }

    #[test]
    fn codex_can_never_report_a_cost() {
        assert_eq!(
            CostQuality::Unavailable,
            CostQuality::Unavailable
        );
    }
}
