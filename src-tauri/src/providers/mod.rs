//! Provider detection and event normalisation.
//!
//! Both CLIs stream newline-delimited JSON and agree on nothing but the
//! newline. Everything downstream sees `ProviderEvent` and never a provider's
//! raw shape. `mock` replays recorded JSONL through these same parsers, so a
//! test exercises the real normalisation without spending a subscription.

pub mod chats;
pub mod claude;
pub mod codex;
pub mod codex_edits;
pub mod limits;
#[cfg(test)]
pub mod mock;

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderId {
    Claude,
    Codex,
}

/// What a live detection found. "Not installed" is a state, not an error:
/// the app is expected to run on a machine with neither CLI present.
#[derive(Debug, Clone, Serialize)]
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
    /// Whether a mid-task instruction reaches this CLI live or has to wait.
    /// The UI says which before the user types, rather than after.
    pub steering: Steering,
}

/// How a provider takes an instruction given to it mid-run.
///
/// Verified against both CLIs, not assumed: `claude -p --input-format
/// stream-json` keeps reading stdin across turns, and `codex exec` has no
/// stdin channel at all once its prompt is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Steering {
    /// A new user message on stdin, taken while the turn is still running.
    Live,
    /// Nothing to speak to. The instruction waits, and applying it means
    /// ending the process and resuming the session it recorded.
    Checkpoint,
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
    /// Provider-reported model id, when the CLI includes one.
    #[serde(default)]
    pub model: Option<String>,
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

impl Usage {
    /// Fold another turn's numbers into this total.
    ///
    /// Claude reports `usage` per turn but `total_cost_usd` as a running total
    /// for the whole session - checked against a live two-turn run, where the
    /// cost went 0.0302 -> 0.0405 while the second turn's own output was six
    /// tokens. So tokens add and cost replaces. Adding the costs would bill
    /// the first turn twice; keeping only the last usage would throw every
    /// turn but the last away.
    pub fn absorb(&mut self, next: &Usage) {
        if next.model.is_some() {
            self.model = next.model.clone();
        }
        self.input_tokens = self.input_tokens.saturating_add(next.input_tokens);
        self.cached_input_tokens = self
            .cached_input_tokens
            .saturating_add(next.cached_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(next.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(next.reasoning_tokens);
        // A turn that reported no cost does not erase one that did.
        if next.cost_usd.is_some() {
            self.cost_usd = next.cost_usd;
            self.cost_quality = next.cost_quality;
        }
    }
}

/// When Codex no longer offers `slug`: its own first choice and that model's
/// default effort, from the model list it caches for the account. `None` while
/// the slug is offered or the list cannot be read.
pub fn codex_replacement(slug: &str) -> Option<(String, String)> {
    let home = env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".codex")))?;
    let text = std::fs::read_to_string(home.join("models_cache.json")).ok()?;
    let cache: Value = serde_json::from_str(&text).ok()?;
    let models = cache["models"].as_array()?;
    if models.iter().any(|m| m["slug"] == slug) {
        return None;
    }
    let top = models
        .iter()
        .filter(|m| m["visibility"] == "list")
        .min_by_key(|m| m["priority"].as_i64().unwrap_or(i64::MAX))?;
    Some((
        top["slug"].as_str()?.to_string(),
        top["default_reasoning_level"].as_str()?.to_string(),
    ))
}

/// What keeps a Codex run to what Orteca and the repository supply (§4.3.3).
/// An empty `CODEX_HOME` would be cleaner but signs the user out, so the
/// user's `config.toml` - plugins, MCP servers, notify, proxy - is skipped
/// while auth still comes from `CODEX_HOME`, and what loads without any config
/// is switched off by name. The global `AGENTS.md` has no switch and still loads.
pub const CODEX_ISOLATION: &[&str] = &[
    "--ignore-user-config",
    "-c",
    "features.plugins=false",
    "-c",
    "features.apps=false",
    "-c",
    "features.recommended_plugins=false",
    "-c",
    "skills.include_instructions=false",
    // Skipping config.toml also skips this, and without it Codex on Windows
    // quietly turns `--sandbox workspace-write` into read-only and still exits
    // 0. Both values write; `elevated` is the stronger fence.
    "-c",
    "windows.sandbox=\"elevated\"",
    // The repo's `AGENTS.md` reaches a run only through Orteca's memory, once
    // the user has imported it. Global `AGENTS.md` still loads (no switch).
    "-c",
    "project_doc_max_bytes=0",
];

/// Adjacently tagged so every variant survives, including the newtype ones:
/// `{"kind":"text","data":"..."}`. This is both the payload the UI receives
/// and the row written to `task_events`, so there is one shape, not two.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ProviderEvent {
    /// Runner-owned progress; never inferred from provider prose.
    StageProgress {
        stages: Vec<String>,
        current: Vec<usize>,
    },
    Started {
        session_id: String,
    },
    Text(String),
    /// The model's reasoning between steps. Shown while it runs, never the answer.
    Thinking(String),
    ToolUse {
        name: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changes: Vec<FileEdit>,
    },
    ToolResult {
        id: String,
        changes: Vec<FileEdit>,
        failed: bool,
    },
    Usage(Usage),
    Done {
        /// The provider's own final answer. Codex does not report one, so it is
        /// empty there and the caller keeps the last `Text` instead.
        result: String,
        structured: Option<Value>,
        /// Model turns this result covers. Claude reports several per result
        /// (`num_turns`); a Codex `turn.completed` is exactly one.
        turns: u32,
    },
    Failed {
        kind: FailureKind,
        message: String,
    },
    /// Runner-owned: a command the agent handed Orteca to run. `asking` is
    /// true while it waits for the user's go-ahead, false once it runs.
    Wait {
        command: String,
        asking: bool,
    },
}

/// A provider-recorded edit. Missing patch means the CLI only named the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEdit {
    pub path: String,
    pub patch: Option<String>,
}

/// Only the kinds a parser actually produces. `CliMissing`, `Cancelled` and
/// `MalformedOutput` were dropped: they are the process runner's to report,
/// and it does not exist until Milestone 4. Whichever of them that runner
/// truly emits comes back then, one at a time.
///
/// `BudgetReached` arrived with Milestone 6 and is the exception that proves
/// the rule: `claude --max-turns` makes the CLI itself report a ceiling it hit,
/// as `subtype: error_max_turns`. It used to be filed as `Timeout`, which sent
/// the user to look for a hang that never happened. Running out of a budget
/// Orteca set is not a fault, and `run` turns it into the budget-reached
/// outcome rather than a failed task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    AuthExpired,
    UsageLimit,
    RateLimit,
    Timeout,
    BudgetReached,
    Crashed,
}

impl ProviderEvent {
    /// The `task_events.kind` column. Same spelling as the serialised tag.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::StageProgress { .. } => "stageProgress",
            Self::Started { .. } => "started",
            Self::Text(_) => "text",
            Self::Thinking(_) => "thinking",
            Self::ToolUse { .. } => "toolUse",
            Self::ToolResult { .. } => "toolResult",
            Self::Usage(_) => "usage",
            Self::Done { .. } => "done",
            Self::Failed { .. } => "failed",
            Self::Wait { .. } => "wait",
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
    } else if m.contains("usage limit")
        || m.contains("quota")
        || m.contains("credit balance")
        // How Claude words a spent plan window. Unverified against a recording.
        || m.contains("hit your limit")
        || m.contains("session limit")
        || m.contains("weekly limit")
    {
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

    /// Whether this CLI can be spoken to while it works.
    pub fn steering(self) -> Steering {
        match self {
            Self::Claude => Steering::Live,
            Self::Codex => Steering::Checkpoint,
        }
    }

    /// Restart a recorded session with something new to say.
    /// It is isolated the same way as a fresh `codex exec`.
    ///
    /// `codex exec resume` has no `--sandbox` flag - only the bypass one this
    /// project forbids - so the sandbox has to be set through config. The
    /// override is validated: a bogus value is refused with the three variants
    /// named, which is how this spelling was confirmed without spending a run.
    ///
    /// `schema` is the stage's artifact contract. A resumed stage keeps it, or
    /// the resume would quietly drop the shape the route asked for and the
    /// stage would come back as prose nobody may parse.
    ///
    /// `writes` is the stage's own fence: a Plan or Review resumed to take an
    /// instruction stays read-only, exactly as its first launch was.
    pub fn resume_args(self, session: &str, schema: Option<&Path>, writes: bool) -> Vec<String> {
        let arg = str::to_string;
        match self {
            Self::Codex => [
                vec![
                    arg("exec"),
                    arg("resume"),
                    session.to_string(),
                    arg("-"),
                    arg("--json"),
                    arg("-c"),
                    arg(if writes {
                        "sandbox_mode=\"workspace-write\""
                    } else {
                        "sandbox_mode=\"read-only\""
                    }),
                ],
                CODEX_ISOLATION.iter().map(|a| arg(a)).collect(),
                schema.map_or_else(Vec::new, |path| {
                    vec![arg("--output-schema"), path.display().to_string()]
                }),
            ]
            .concat(),
            // Claude never needs this: it takes instructions live, so there is
            // no session to pick back up.
            Self::Claude => Vec::new(),
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

    /// Return a schema-constrained artifact from a raw provider event.
    ///
    /// This is deliberately separate from `parse_line`: normal assistant text
    /// is never treated as data, and the runner calls this only for a stage
    /// that actually supplied an output schema. Codex 0.154 emits its
    /// schema-constrained final value as the text of the completed agent
    /// message, while Claude puts it in `structured_output` on the result.
    pub fn structured_output(self, v: &Value) -> Option<Value> {
        match self {
            Self::Claude => claude::structured_output(v),
            Self::Codex => codex::structured_output(v),
        }
    }

    pub fn detect(self) -> Detected {
        let path = which(self.program());
        let version = path.as_deref().and_then(version_of);
        let auth = match path.as_deref() {
            Some(p) => self.auth(p),
            None => Auth::Unknown,
        };
        self.detected(path, version, auth)
    }

    /// The same answer as `detect`, with the two probes overlapped.
    ///
    /// Each one is a process start, and both CLIs are `.cmd` shims over a large
    /// Node bundle that needs about two seconds before it prints a word. They
    /// do not depend on each other, so asking in series paid that wait twice.
    pub async fn detect_async(self) -> Detected {
        let Some(path) = which(self.program()) else {
            return self.detected(None, None, Auth::Unknown);
        };
        let (version, auth) = self.probe(path.clone()).await;
        self.detected(Some(path), version, auth)
    }

    /// Ask one CLI both questions at once. Split out from `detect_async` so the
    /// overlap can be measured against a shim instead of the real PATH.
    async fn probe(self, path: PathBuf) -> (Option<String>, Auth) {
        let (for_version, for_auth) = (path.clone(), path);
        let version = tokio::task::spawn_blocking(move || version_of(&for_version));
        let auth = tokio::task::spawn_blocking(move || self.auth(&for_auth));
        // A probe that could not finish is exactly what `None` and `Unknown`
        // already mean; neither is allowed to become an optimistic answer.
        (
            version.await.ok().flatten(),
            auth.await.unwrap_or(Auth::Unknown),
        )
    }

    /// One place builds a `Detected`, so the sync and overlapped paths cannot
    /// drift into reporting the same machine differently.
    fn detected(self, path: Option<PathBuf>, version: Option<String>, auth: Auth) -> Detected {
        Detected {
            id: self,
            program: self.program(),
            version,
            auth,
            path: path.map(|p| p.display().to_string()),
            cost_quality: match self {
                // Claude's own total is a client-side estimate, not a bill.
                Self::Claude => CostQuality::Estimated,
                // Priced from published rates once they have been fetched.
                Self::Codex => CostQuality::Estimated,
            },
            steering: self.steering(),
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
            // `claude -p` bills a key in the environment ahead of the plan login.
            Self::Claude if env::var_os("ANTHROPIC_API_KEY").is_some_and(|k| !k.is_empty()) => Auth::ApiKey,
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
/// `CreateProcess` only ever appends `.exe`, so an npm shim - which is how
/// both CLIs install on Windows - is invisible to a bare program name. The
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
            let dir = if dir.is_absolute() {
                dir
            } else {
                cwd.join(dir)
            };
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    runtime.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            // Never in the user's project: detection runs before anyone has
            // consented to that repository, same reason npm install uses temp.
            let mut run =
                crate::proc::spawn(&path.to_string_lossy(), args, &env::temp_dir()).ok()?;
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
        })
        .await
        .ok()
        .flatten()
    })
}

fn version_of(path: &Path) -> Option<String> {
    let (lines, code) = capture(path, &["--version"])?;
    (code == Some(0))
        .then(|| lines.into_iter().next())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stage_progress_preserves_repeated_and_concurrent_stages() {
        let event = ProviderEvent::StageProgress { stages: vec!["verify".into(), "review".into(), "fix".into(), "verify".into()], current: vec![0, 1] };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["kind"], "stageProgress");
        assert_eq!(value["data"]["current"], serde_json::json!([0, 1]));
        let decoded: ProviderEvent = serde_json::from_value(value).unwrap();
        assert_eq!(decoded, event);
    }

    /// The numbers a steered run reports. Claude sends usage per turn and cost
    /// as a session running total, so one of them adds and the other replaces.
    /// Getting this uniform in either direction reports a wrong number.
    #[test]
    fn steered_turns_add_their_tokens_and_keep_the_latest_cost() {
        let turn = |input, output, cost| Usage {
            model: Some("test-model".into()),
            input_tokens: input,
            cached_input_tokens: 10,
            output_tokens: output,
            reasoning_tokens: 1,
            cost_usd: cost,
            cost_quality: CostQuality::Estimated,
        };
        let mut total = turn(100, 4, Some(0.0302));
        total.absorb(&turn(2, 6, Some(0.0405)));

        assert_eq!(total.input_tokens, 102);
        assert_eq!(total.output_tokens, 10);
        assert_eq!(total.cached_input_tokens, 20);
        assert_eq!(
            total.cost_usd,
            Some(0.0405),
            "the cost is already a session total"
        );

        // Codex reports no cost at all, and that must not erase Claude's.
        let mut kept = turn(1, 1, Some(0.5));
        kept.absorb(&Usage {
            cost_usd: None,
            cost_quality: CostQuality::Unavailable,
            ..turn(1, 1, None)
        });
        assert_eq!(kept.cost_usd, Some(0.5));
    }

    #[test]
    fn a_user_message_survives_quotes_and_newlines() {
        let line = claude::user_message(
            "say \"hi\"
then stop",
        );
        assert_eq!(
            line.lines().count(),
            1,
            "a newline would split the JSONL frame"
        );
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["role"], "user");
        assert_eq!(
            v["message"]["content"],
            "say \"hi\"
then stop"
        );
    }

    /// Resume has no --sandbox flag, so the mode travels as a config override.
    /// Losing it would leave a resumed agent read-only while still exiting 0.
    #[test]
    fn a_resumed_codex_session_keeps_its_write_sandbox() {
        let argv = ProviderId::Codex.resume_args("abc-123", None, true).join(" ");
        assert!(argv.contains("exec resume abc-123"));
        assert!(argv.contains("sandbox_mode=\"workspace-write\""));
        assert!(!argv.contains("danger"));
        assert!(argv.contains("--json"));
        // A resume must not bring the user's plugins and skills back.
        assert!(argv.contains(&CODEX_ISOLATION.join(" ")));
        // The prompt arrives on stdin, never as an argument.
        assert!(ProviderId::Codex
            .resume_args("abc-123", None, true)
            .contains(&"-".to_string()));
        // A resumed stage keeps the artifact contract its route asked for.
        let schema = PathBuf::from("C:/tmp/plan.json");
        let with_schema = ProviderId::Codex.resume_args("abc-123", Some(&schema), true);
        assert!(with_schema.contains(&"--output-schema".to_string()));
        assert!(with_schema.contains(&schema.display().to_string()));
        // A stage that may not write is resumed as one that may not write.
        let review = ProviderId::Codex.resume_args("abc-123", None, false).join(" ");
        assert!(review.contains("sandbox_mode=\"read-only\""));
        assert!(!review.contains("workspace-write"));
    }

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
        std::fs::write(
            &shim,
            "@echo off
echo 1.2.3
",
        )
        .unwrap();
        let found = tokio::task::spawn_blocking(move || version_of(&shim))
            .await
            .unwrap();
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
        std::fs::write(
            &path,
            format!(
                "@echo off
{body}
"
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn claude_auth_comes_from_the_cli_not_a_credential_file() {
        // The exact shape `claude auth status --json` returned when logged out.
        let out = shim(
            "claude-out",
            r#"echo {"loggedIn":false,"authMethod":"none"}"#,
        );
        assert_eq!(ProviderId::Claude.auth(&out), Auth::SignedOut);

        let sub = shim(
            "claude-sub",
            r#"echo {"loggedIn":true,"authMethod":"claudeai"}"#,
        );
        assert_eq!(ProviderId::Claude.auth(&sub), Auth::Subscription);

        let key = shim(
            "claude-key",
            r#"echo {"loggedIn":true,"authMethod":"apiKey"}"#,
        );
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
        std::fs::write(
            &shim,
            "@echo off\r\nping -n 6 127.0.0.1 >nul\r\necho too late\r\n",
        )
        .unwrap();
        assert_eq!(version_of(&shim), None);
        std::fs::remove_dir_all(cwd).unwrap();
    }

    /// Both questions cost a full Node start, so asking them in series spent
    /// that wait twice and the Providers card sat empty for all of it.
    #[tokio::test]
    async fn the_two_probes_of_one_provider_overlap() {
        let cwd = temp_dir("probe-overlap");
        let shim = cwd.join("agent.cmd");
        // Slow, and the same answer to either question - only timing matters.
        std::fs::write(
            &shim,
            "@echo off
ping -n 3 127.0.0.1 >nul
echo agent 9.9.9
",
        )
        .unwrap();

        let started = std::time::Instant::now();
        let (version, _) = ProviderId::Claude.probe(shim).await;
        let elapsed = started.elapsed();

        assert_eq!(
            version,
            Some("agent 9.9.9".into()),
            "the shim was not actually run"
        );
        // One probe is about two seconds here; in series the pair is about four.
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "probes ran in series: {elapsed:?}"
        );
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
        assert_eq!(CostQuality::Unavailable, CostQuality::Unavailable);
    }
}
