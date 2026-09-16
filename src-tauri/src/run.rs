//! Running a route: prompt -> stages -> streams -> diff -> result.
//!
//! The route and its model choices are decided in `routing` before anything here
//! starts a process. This module's job is to honour them: run each stage in
//! turn, normalise every line through the provider's parser, write each event
//! to the append-only log, let the user steer and stop it, and end with a diff
//! they can check.
//!
//! A trivial task's route is one Implement stage, so that path is exactly the
//! single-stage run Milestone 4 shipped, with a budget attached.
//!
//! A failed Verify can be fixed and checked again until it passes or a Fix
//! makes no progress. An independent Review runs once; its Fix is judged by
//! Verify instead of buying another reviewer.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use crate::error::{AppError, ErrorKind};
use crate::proc::{self, Line};
use crate::project::{self, FileStat};
use crate::providers::{
    claude, codex, CostQuality, FailureKind, ProviderEvent, ProviderId, Steering, Usage,
};
use crate::routing::{self, Route, Stage, StageNote};
use crate::store::{Baseline, Store};

// Commands the agent may never run, in either shell. Deny beats allow, so these
// hold even though `--allowedTools` grants Bash and PowerShell outright.
// Scoped rules match command spelling, not every way of executing Git: this is a
// guardrail against an agent going wrong, not a sandbox against a hostile one.
// Claude exposes no OS-level sandbox flag the way `codex --sandbox` does.
const CLAUDE_DENY_COMMANDS: &[&str] = &[
    // Rewriting or publishing the user's history. Orteca reads git, never rewrites it.
    "git push:*",
    "git reset:*",
    "git clean:*",
    "git rebase:*",
    "git restore:*",
    "git checkout --:*",
    "git filter-branch:*",
    // Destroying files outside a normal edit.
    "rm:*",
    "rmdir:*",
    "del:*",
    "rd:*",
    "Remove-Item:*",
    // Publishing under the user's name.
    "npm publish:*",
    "cargo publish:*",
    "gh release:*",
    // Reaching the network, which is how a bad instruction exfiltrates a repo.
    "curl:*",
    "wget:*",
    "Invoke-WebRequest:*",
    "Invoke-RestMethod:*",
    "scp:*",
    "ssh:*",
    // Touching the machine rather than the project.
    "shutdown:*",
    "reg:*",
    "schtasks:*",
    "net user:*",
    "Set-ExecutionPolicy:*",
];

/// Tools that edit files. A stage that is not meant to write is denied them
/// outright rather than merely asked not to: Claude has no read-only mode to
/// set, and a Plan stage that edited the code would have skipped the Review the
/// route put after it.
const CLAUDE_EDIT_TOOLS: &[&str] = &["Edit", "Write", "NotebookEdit", "MultiEdit"];

/// Every denied command in both shells Claude can reach, so a blocked command
/// cannot simply be rerun through the other one. A non-writing stage also loses
/// the edit tools.
fn claude_deny(writes: bool) -> Vec<String> {
    let commands = CLAUDE_DENY_COMMANDS
        .iter()
        .flat_map(|cmd| [format!("Bash({cmd})"), format!("PowerShell({cmd})")]);
    if writes {
        commands.collect()
    } else {
        commands
            .chain(CLAUDE_EDIT_TOOLS.iter().map(|t| (*t).to_string()))
            .collect()
    }
}

/// Claude has no read-only sandbox. Non-writing stages therefore receive a
/// read/check allowlist instead of broad shell access; denying Edit/Write alone
/// is not enough because a shell command can write the same file.
fn claude_allowed(plan: &StagePlan) -> Vec<String> {
    let stage = plan.stage;
    if stage.writes() {
        let tools: &[&str] = if plan.shell {
            &["Bash", "PowerShell"]
        } else {
            &["Read", "Edit", "Write", "Glob", "Grep"]
        };
        return tools.iter().map(|t| (*t).to_string()).collect();
    }

    let mut tools = vec![
        "Read".to_string(),
        "Grep".to_string(),
        "Glob".to_string(),
        // Exact commands, not prefixes: `git diff --output=<file>` writes.
        "Bash(git diff)".to_string(),
        "Bash(git diff --stat)".to_string(),
        "Bash(git status)".to_string(),
        "Bash(git status --short)".to_string(),
    ];
    if stage == Stage::Verify {
        // Not read-only, and not claimed to be: these run the repository's own
        // scripts, which the user consented to when trusting the project. What
        // they write still lands in the run's diff. Plan and Review get none.
        tools.extend(
            [
                "Bash(npm test *)",
                "Bash(npm run build *)",
                "Bash(node --test *)",
                "Bash(cargo test *)",
                "Bash(cargo clippy *)",
                "Bash(composer test *)",
                "Bash(php vendor/bin/phpunit *)",
                "Bash(go test *)",
                "Bash(python -m pytest *)",
            ]
            .into_iter()
            .map(str::to_string),
        );
    }
    tools
}

/// What a user can still do to a run that is already going.
#[derive(Debug)]
pub enum Control {
    /// Stop now. Whatever the agent already wrote to the working tree stays
    /// written: Orteca captures a diff, it never reverts the user's files.
    Cancel,
    /// Words for the agent while it works. Where they go depends on the
    /// provider: a live one takes them mid-turn, a checkpoint one holds them.
    /// `apply_now` is the user choosing not to wait for a boundary that a
    /// single-stage run never reaches - it ends the process and resumes the
    /// session carrying the instruction.
    Instruct {
        text: String,
        apply_now: bool,
        reply: oneshot::Sender<InstructionReceipt>,
    },
}

/// What actually happened to an instruction, returned only after the run loop
/// has handled it. Enqueueing a control message is not proof that stdin took it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstructionDisposition {
    Live,
    Held,
    Resumed,
    TooLate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstructionReceipt {
    pub disposition: InstructionDisposition,
}

/// The runs a user can still reach, one sender per live task.
///
/// The sender is dropped the moment its run ends, so a control aimed at a task
/// that has already finished is refused rather than quietly going nowhere -
/// a Stop button that reports success while an agent keeps editing would be
/// the worst kind of lie this app can tell.
#[derive(Default)]
pub struct Live(Mutex<HashMap<i64, mpsc::UnboundedSender<Control>>>);

impl Live {
    /// Register a run and hand back the end `stream` listens on.
    fn open(&self, task_id: i64) -> mpsc::UnboundedReceiver<Control> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.0
            .lock()
            .expect("live runs poisoned")
            .insert(task_id, tx);
        rx
    }

    fn close(&self, task_id: i64) {
        self.0.lock().expect("live runs poisoned").remove(&task_id);
    }

    pub fn send(&self, task_id: i64, control: Control) -> crate::error::Result<()> {
        let delivered = self
            .0
            .lock()
            .expect("live runs poisoned")
            .get(&task_id)
            .is_some_and(|tx| tx.send(control).is_ok());
        if delivered {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorKind::NotFound,
                "That run has already finished.",
            ))
        }
    }

    /// Deliver an instruction and wait for the run loop's real disposition.
    pub async fn instruct(
        &self,
        task_id: i64,
        text: String,
        apply_now: bool,
    ) -> crate::error::Result<InstructionReceipt> {
        let (reply, answer) = oneshot::channel();
        self.send(
            task_id,
            Control::Instruct {
                text,
                apply_now,
                reply,
            },
        )?;
        answer.await.map_err(|_| {
            AppError::new(
                ErrorKind::NotFound,
                "That run ended before it could confirm the instruction.",
            )
        })
    }
}

/// The `task_events` row a stop leaves behind. Not a `ProviderEvent`: the
/// provider did not say this, the user did. Without it a run stopped two
/// seconds in is indistinguishable afterwards from one that died on its own.
const CANCEL_PAYLOAD: &str = r#"{"kind":"cancel","data":{"by":"user"}}"#;

/// Why a run stopped short of its route.
///
/// Not a failure and not a success: the work that was done is real, the diff is
/// kept, and the usage is recorded. What it is *not* is finished, and Orteca
/// does not decide on the user's behalf to spend more.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetStop {
    /// Which check a Fix could not move: `review` or `verify`. Older rows also
    /// carry `calls`, `turns` and `tokens`, from when routes had ceilings.
    pub limit: &'static str,
    pub allowed: u64,
    pub observed: u64,
    /// The stages the route still had. Starting them is a deliberate choice the
    /// user makes; nothing here does it for them.
    pub remaining: Vec<Stage>,
    pub message: String,
}

/// A session a follow-up can pick back up. A provider's cache is per model
/// and cools a few minutes after the last call, so both travel with it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resume {
    pub session: String,
    pub model: String,
    /// When the session's last call ended, in Unix milliseconds.
    pub ended_at: u64,
    /// The user's new words. The session already holds everything before them.
    #[serde(default, skip_serializing)]
    pub reply: String,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// What the UI gets when the run ends.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub task_id: i64,
    /// `done`, `cancelled`, `failed`, `budgetReached`, `reviewRejected` or
    /// `verifyFailed`.
    pub status: &'static str,
    /// The provider's final answer, or its last message if it reports no final
    /// field. Empty is possible and is not an error.
    pub summary: String,
    pub failure: Option<String>,
    /// Why it failed, when it did. `usageLimit` is what offers the other CLI.
    pub failure_kind: Option<FailureKind>,
    /// `None` when the run ended before the provider reported any numbers.
    /// The UI must say "unavailable" and never print a zero.
    pub usage: Option<Usage>,
    pub diff: Vec<FileStat>,
    /// Reviewable Git patch, if Git could produce one.
    pub patch_text: Option<String>,
    /// JSONL records that the provider parser did not recognise.
    pub unknown_events: u32,
    pub duration_ms: u64,
    /// The diff includes edits that were already in the working tree.
    pub dirty_at_start: bool,
    /// The route this run was given before any provider started, tiers and
    /// classifier signals included.
    pub route: Route,
    /// Stages that actually ran, in order, with any validated artifact.
    pub stages: Vec<StageNote>,
    /// Provider processes started, stages and resumes together.
    pub calls_used: u32,
    /// Provider turns that completed.
    pub turns_used: u32,
    /// Set when a Fix changed nothing and a check still did not pass.
    pub budget_stop: Option<BudgetStop>,
    /// Comparable finished runs in this project, once there are five. The UI
    /// shows no savings figure without it.
    pub baseline: Option<Baseline>,
    /// The separate copy the run worked in, if the user asked for one.
    pub worktree: Option<project::Worktree>,
    /// What a follow-up would resume. `None` for a copy, whose folder a
    /// follow-up does not run in.
    pub resume: Option<Resume>,
}

/// What one stage asks of its CLI, beyond the prompt.
#[derive(Debug, Clone)]
pub struct StagePlan {
    pub stage: Stage,
    /// The artifact contract, already written to a file. Claude takes the
    /// schema text inline, Codex takes the path.
    pub schema: Option<PathBuf>,
    /// The route's tier, which names the default model and effort.
    pub tier: routing::Tier,
    /// Stage-specific model override. Efficient Review can use a different
    /// provider tier when the benchmark says it is enough.
    pub model: Option<&'static str>,
    /// Replaces the tier's effort for a benchmarked Plan or Review.
    pub effort: Option<&'static str>,
    /// False for an Implement that Orteca's own tests follow: Claude then gets
    /// no Bash or PowerShell, two fewer tool schemas on every turn. Codex's only
    /// tool is its shell, so it ignores this.
    pub shell: bool,
}

impl StagePlan {
    fn model(&self, id: ProviderId) -> routing::ModelChoice {
        let mut choice = self.tier.model(id);
        if let Some(model) = self.model {
            choice.model = model;
        }
        if let Some(effort) = self.effort {
            choice.effort = effort;
        }
        choice
    }
}

/// The argv for one stage.
///
/// Claude is never given `--bare`: that would disable OAuth and force an API
/// key, billing the user instead of using the subscription they already have.
/// Codex is never given `--sandbox danger-full-access`; `workspace-write` is
/// the widest access Orteca asks for, and a stage that is not meant to edit
/// gets `read-only` instead.
///
/// The schema flags were checked against the installed CLIs on 2026-09-12
/// rather than taken from the spec. No turn ceiling is passed: a stage runs
/// until it is done (§4.3.8).
pub fn args(id: ProviderId, plan: &StagePlan) -> Vec<String> {
    let arg = str::to_string;
    let writes = plan.stage.writes();
    match id {
        ProviderId::Codex => [
            vec![arg("exec"), arg("-"), arg("--json")],
            crate::providers::CODEX_ISOLATION
                .iter()
                .map(|a| arg(a))
                .collect(),
            model_args(id, plan),
            vec![
                arg("--sandbox"),
                // A stage with no business editing cannot edit. Codex has an
                // OS-level fence for this; using it is cheaper and more certain
                // than asking the agent nicely.
                arg(if writes {
                    "workspace-write"
                } else {
                    "read-only"
                }),
            ],
            plan.schema.as_deref().map_or_else(Vec::new, |path| {
                vec![arg("--output-schema"), path.display().to_string()]
            }),
        ]
        .concat(),
        ProviderId::Claude => {
            let mut args = vec![
                arg("-p"),
                arg("--output-format"),
                arg("stream-json"),
                arg("--verbose"),
                // Only what Orteca and the repository supply (§4.3.3). User
                // settings carry the user's plugins, hooks and CLAUDE.md; the
                // repo's project settings, hooks included, still load, which
                // is why consent stays unconditional.
                arg("--setting-sources"),
                arg("project,local"),
                // No MCP server at all: the user's, the claude.ai connectors,
                // and the repository's `.mcp.json` alike.
                arg("--strict-mcp-config"),
                // Skills load as slash commands; this is what drops them.
                arg("--disable-slash-commands"),
                // Without the user's settings every built-in tool schema loads
                // in full. No stage uses the rest; the grants below narrow these.
                arg("--tools"),
                arg(if plan.shell {
                    "Bash,PowerShell,Read,Edit,Write,Glob,Grep"
                } else {
                    "Read,Edit,Write,Glob,Grep"
                }),
                arg("--permission-mode"),
                arg("acceptEdits"),
                // Streaming input is what makes a mid-task instruction possible:
                // the prompt goes in as a user message and the process keeps
                // reading, so another one can follow while the turn is running.
                // The run then ends when Orteca closes stdin, not at the first
                // result. Verified against the CLI, including a second turn.
                arg("--input-format"),
                arg("stream-json"),
                // Nothing is listening for a permission prompt. With the default
                // `host` target a headless run hangs forever waiting for an answer
                // no one can give; `none` denies instead, so the agent is told no
                // and carries on.
                arg("--permission-prompts"),
                arg("none"),
                arg("--allowedTools"),
            ];
            args.extend(claude_allowed(plan));
            // The denylist narrows the write-stage shell grant and removes
            // native edit tools from every non-writing stage.
            args.push(arg("--disallowedTools"));
            args.extend(claude_deny(writes));
            args.extend(schema_arg(plan));
            args.extend(model_args(id, plan));
            args
        }
    }
}

/// The model and effort a tier asks for. All four flags answer "argument
/// missing" when given no value (§4.3.4). Codex has no effort flag; the config
/// key is the one its own `config.toml` uses.
fn model_args(id: ProviderId, plan: &StagePlan) -> Vec<String> {
    let choice = plan.model(id);
    match id {
        ProviderId::Claude => vec![
            "--model".into(),
            choice.model.into(),
            "--effort".into(),
            choice.effort.into(),
        ],
        ProviderId::Codex => vec![
            "--model".into(),
            choice.model.into(),
            "-c".into(),
            format!("model_reasoning_effort=\"{}\"", choice.effort),
        ],
    }
}

/// Claude takes the schema as text on the command line, not as a path.
fn schema_arg(plan: &StagePlan) -> Vec<String> {
    plan.stage
        .schema()
        .filter(|_| plan.schema.is_some())
        .map_or_else(Vec::new, |schema| {
            vec!["--json-schema".to_string(), schema.to_string()]
        })
}

/// Write a stage's artifact contract where the CLI can read it.
///
/// Codex needs a file, so one is written for both providers and the presence of
/// the file is what says "this stage has a contract". It lives in the temp
/// directory Orteca owns, never in the user's repository - a schema file that
/// showed up in their diff would be Orteca editing their project.
fn write_schema(task_id: i64, stage: Stage) -> Option<PathBuf> {
    let schema = stage.schema()?;
    let dir = proc::owned_temp().unwrap_or_else(std::env::temp_dir);
    let path = dir.join(format!("orteca-schema-{task_id}-{}.json", stage.name()));
    std::fs::write(&path, schema).ok()?;
    Some(path)
}

/// Everything worth keeping from a stream of events.
#[derive(Default)]
struct Outcome {
    /// Codex reports no final-answer field, so its last message is the answer.
    last_text: String,
    result: String,
    /// Summed across turns. A steered run reports usage once per turn, so
    /// keeping only the last would count one turn and throw the rest away.
    /// `Usage::absorb` knows which fields add and which replace.
    usage: Option<Usage>,
    /// What earlier provider processes cost, added up. See `begin_process`.
    banked_cost: Option<f64>,
    /// A Codex turn ran on a model with no known price. See `price_codex`.
    unpriced: bool,
    failure: Option<String>,
    /// The kind a provider reported with its failure. A specific kind outlives
    /// a later generic one: Claude's spent plan arrives as a rate-limit event,
    /// and the result after it may be worded as nothing in particular.
    reported_kind: Option<FailureKind>,
    /// A provider reported a result at least once. With a live provider that
    /// is once per turn, so it does not mean the run is over.
    done: bool,
    /// The run reached its own end: the last turn finished and Orteca closed
    /// stdin, or a checkpoint provider exited by itself. This is what
    /// separates a run stopped mid-work from one that had already answered.
    finished: bool,
    /// The user stopped this run. Not a failure, and not a success either.
    cancelled: bool,
}

impl Outcome {
    /// Clear what belonged to the stage that just ended. Usage, failures and
    /// the cancelled flag all belong to the task and survive.
    fn begin_stage(&mut self) {
        self.last_text.clear();
        self.result.clear();
        self.done = false;
        self.finished = false;
    }

    /// Claude's `total_cost_usd` is a running total for one process, so within
    /// a process the latest report replaces the last (`Usage::absorb`). A new
    /// process counts from zero again. Whatever the previous one reached is
    /// banked first, or a three-stage run would report one stage's cost.
    fn begin_process(&mut self) {
        if let Some(cost) = self.usage.as_mut().and_then(|u| u.cost_usd.take()) {
            *self.banked_cost.get_or_insert(0.0) += cost;
        }
    }

    /// Put every process's cost, added up, on the task's usage.
    fn settle_cost(&mut self) {
        self.begin_process();
        if let Some(usage) = self.usage.as_mut() {
            usage.cost_usd = self.banked_cost;
            if self.unpriced {
                usage.cost_usd = None;
                usage.cost_quality = CostQuality::Unavailable;
            }
        }
    }

    fn exited(&mut self, id: ProviderId, code: Option<i32>, noise: &[String]) {
        // A run the user killed has no exit code worth reading: the tree was
        // terminated, so "did not finish" is the expected outcome, not a fault.
        if self.cancelled {
            return;
        }
        if self.failure.is_none() && (code != Some(0) || !self.done) {
            self.failure = Some(exit_message(id, code, noise));
        }
    }

    fn absorb(&mut self, event: &ProviderEvent) {
        match event {
            ProviderEvent::Text(text) => self.last_text = text.clone(),
            ProviderEvent::Usage(usage) => match &mut self.usage {
                Some(total) => total.absorb(usage),
                None => self.usage = Some(usage.clone()),
            },
            ProviderEvent::Done { result, .. } => {
                self.done = true;
                self.result = result.clone();
            }
            ProviderEvent::Failed { kind, message } => {
                self.failure = Some(message.clone());
                if *kind != FailureKind::Crashed || self.reported_kind.is_none() {
                    self.reported_kind = Some(*kind);
                }
            }
            _ => {}
        }
    }

    /// Why the run failed: the kind a provider reported, else read from the message.
    fn failure_kind(&self) -> Option<FailureKind> {
        self.failure.as_deref().map(|message| {
            self.reported_kind
                .unwrap_or_else(|| crate::providers::classify_failure(message))
        })
    }

    /// What the task row and the result screen both call this run. A stop that
    /// lands after the provider already answered does not rewrite the answer.
    fn status(&self) -> &'static str {
        if self.failure.is_some() {
            "failed"
        } else if self.cancelled && !self.finished {
            "cancelled"
        } else {
            "done"
        }
    }

    fn summary(&self) -> String {
        if self.result.is_empty() {
            self.last_text.clone()
        } else {
            self.result.clone()
        }
    }
}

/// Stream one CLI to completion. A failure is reported as a failed task, not
/// returned: the task row already exists and the UI is already listening.
pub struct Request {
    pub task_id: i64,
    /// The provider every stage runs on.
    ///
    /// One provider for the whole route in Milestone 6. The architecture maps
    /// capabilities to providers, and the route records what each stage would
    /// have preferred, but acting on it means sending a stage to a CLI the user
    /// may not have signed into - which fails a run for a reason the screen
    /// never mentioned. That needs per-stage auth state the router cannot see
    /// yet, so it waits for Milestone 7.
    pub id: ProviderId,
    pub program: PathBuf,
    pub dir: PathBuf,
    /// What the user typed. Each stage gets a brief built from it, never this.
    pub prompt: String,
    pub route: Route,
    pub base_commit: Option<String>,
    pub dirty_at_start: bool,
    /// What was already changed before the run, so its diff can say which
    /// files are not its work. `None` if git could not say.
    pub before_run: Option<project::Snapshot>,
    /// Directory for this run's raw JSONL, one file per stage. `None` records
    /// nothing.
    pub recordings: Option<PathBuf>,
    /// The separate copy `dir` points into, when the user asked for one. Its
    /// changes are committed to the copy's branch when the run ends.
    pub worktree: Option<project::Worktree>,
    /// What the call that read the prompt reported, usage included. Empty when
    /// no such call ran.
    pub classified: Vec<ProviderEvent>,
    /// Files and folders the user attached, already checked to exist. Named
    /// in every brief; Claude also needs their folders granted to read them.
    pub attachments: Vec<PathBuf>,
    /// A follow-up: the session to pick back up, if its model is still the one
    /// the first stage asks for.
    pub resume: Option<Resume>,
    /// A reply that continues `task_id` rather than opening a task of its own:
    /// its usage adds to the task's, and its prompt is logged as a new turn.
    pub continued: bool,
}

/// The raw event stream of one run, kept so a paid run can be replayed free.
///
/// A live run spends the user's subscription; replaying one costs nothing. The
/// bundled fixtures are hand-written guesses that have already been wrong once:
/// they missed the item types Codex reports its own tool failures in, and only
/// a recording fixes that for good.
///
/// Only JSON lines are kept, so the file stays loadable by `mock::replay`,
/// which treats a malformed line as a broken fixture rather than as output.
struct Recording {
    path: Option<PathBuf>,
    file: Option<std::fs::File>,
}

impl Recording {
    /// One file per stage, so a multi-stage route leaves one loadable fixture
    /// per provider process rather than several runs interleaved in one file.
    fn new(dir: Option<&Path>, task_id: i64, stage: Stage, id: ProviderId) -> Self {
        Self {
            path: dir.map(|dir| {
                dir.join(format!(
                    "task-{task_id}-{}-{}.jsonl",
                    stage.name(),
                    id.program()
                ))
            }),
            file: None,
        }
    }

    /// Opened on the first event, so a run that produced none leaves no file.
    /// Nothing here may fail the run: the record that matters is `task_events`,
    /// and a development aid must never cost the user a run they paid for. A
    /// write that fails gives up for the rest of the run rather than retrying.
    fn write(&mut self, value: &serde_json::Value) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.file.is_none() {
            let opened = std::fs::create_dir_all(path.parent().unwrap_or(&path))
                .and_then(|()| std::fs::File::create(&path));
            match opened {
                Ok(file) => self.file = Some(file),
                Err(_) => {
                    self.path = None;
                    return;
                }
            }
        }
        // serde_json orders keys; the parsers read fields by name and do not
        // care. What has to survive is the shape, not the byte order.
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if std::io::Write::write_all(
            file,
            format!(
                "{value}
"
            )
            .as_bytes(),
        )
        .is_err()
        {
            self.path = None;
            self.file = None;
        }
    }
}

/// One process start: what to run, and the first thing to say to it.
struct Launch {
    argv: Vec<String>,
    /// Written to stdin before anything else. Claude wants a stream-json user
    /// message; Codex wants the raw prompt that `exec -` reads.
    opening: String,
}

impl Launch {
    fn first(id: ProviderId, brief: &str, plan: &StagePlan) -> Self {
        Launch {
            argv: args(id, plan),
            opening: match id {
                ProviderId::Claude => claude::user_message(brief),
                ProviderId::Codex => brief.to_string(),
            },
        }
    }

    /// Pick a recorded session back up with everything the user has said since.
    /// The session already holds the history, so only the new words are sent.
    fn resume(id: ProviderId, session: &str, held: &[String], plan: &StagePlan) -> Self {
        let mut argv = id.resume_args(session, plan.schema.as_deref());
        // Without it a resumed session runs on the account's default model.
        if id == ProviderId::Codex {
            argv.extend(model_args(id, plan));
        }
        Launch {
            argv,
            opening: held.join("\n"),
        }
    }

    /// A Fix continues the session that wrote the change, which already holds
    /// the task and every file it read. Claude also needs this for its Edit
    /// tool, which refuses a file it has not Read in the same session.
    fn fix(id: ProviderId, session: &str, brief: &str, plan: &StagePlan) -> Self {
        match id {
            ProviderId::Codex => Self::resume(id, session, &[brief.to_string()], plan),
            ProviderId::Claude => {
                let mut launch = Self::first(id, brief, plan);
                launch.argv.extend(["--resume".into(), session.into()]);
                launch
            }
        }
    }
}

/// What a task is for as long as it runs. Separate from `State` because none
/// of it changes when the provider is restarted.
struct Context {
    task_id: i64,
    id: ProviderId,
    program: PathBuf,
    dir: PathBuf,
    /// The stage running right now, and what it asks of the CLI. Replaced at
    /// each stage boundary; unchanged by a restart inside one.
    plan: StagePlan,
    /// Whether another stage follows this one. It decides where a held
    /// instruction is delivered: with a stage still to come there is a brief to
    /// merge it into, and resuming this one as well would pay twice to say the
    /// same thing.
    final_stage: bool,
    attachments: Vec<PathBuf>,
}

impl Context {
    fn stage(&self) -> &'static str {
        self.plan.stage.name()
    }
}

/// Everything one task carries across a restart. A resumed Codex session is
/// still the same task: the same event log, the same token total, the same
/// diff baseline, the same recording.
struct State {
    outcome: Outcome,
    recording: Recording,
    /// Provider processes started so far, stages and resumes together.
    calls_used: u32,
    /// Completed turns across the whole task.
    turns_used: u32,
    /// Every instruction the user has given, in order. Unlike `held` this is
    /// never cleared: each later stage's brief repeats all of them, so an
    /// instruction never silently expires.
    constraints: Vec<String>,
    /// What each finished stage handed on.
    notes: Vec<StageNote>,
    /// The structured artifact this stage returned, before it is validated.
    structured: Option<serde_json::Value>,
    /// Set when a Fix changed nothing and its check still failed. No later
    /// stage starts after this.
    budget_stop: Option<BudgetStop>,
    /// The route is finished early, and not because anything went wrong: a
    /// Fix changed nothing, so another round has nothing new to try. What
    /// happens next is the user's call.
    halt: bool,
    /// The session that wrote the change. A Fix resumes it, so the task
    /// and the files it already read stay in its cached context.
    work_session: Option<String>,
    /// Whether the last Fix changed the tree, until a check passes. A check
    /// that still fails after a Fix that changed nothing ends the run.
    fix_changed: Option<bool>,
    /// The running totals Codex last reported for each thread, which is what
    /// a resumed process's report is measured from.
    thread_totals: HashMap<String, Usage>,
    /// Non-JSON output - the only clue a CLI leaves when it dies badly.
    noise: Vec<String>,
    /// JSONL records that had no known provider event shape.
    unknown_events: u32,
    /// Instructions the provider has not taken yet. A checkpoint provider
    /// accumulates these; a live one never holds anything.
    held: Vec<String>,
    /// The provider's own session id, which is what a resume needs.
    session: Option<String>,
    /// Apply-now arrived before Codex identified its session. Restart as soon
    /// as the Started event supplies the ID instead of dropping the request.
    apply_now_pending: bool,
    /// The last writing or answering session, for a follow-up.
    resume_point: Option<Resume>,
}

/// What happens after one process ends.
enum Next {
    /// The task is over, however it ended.
    Ended,
    /// The user asked for an instruction to apply now, so the recorded session
    /// is picked back up carrying it.
    Restart(Launch),
}

pub async fn stream(
    store: &Store,
    live: &Live,
    request: Request,
    emit: impl Fn(&ProviderEvent) -> crate::error::Result<()>,
) -> TaskResult {
    let started_at = std::time::Instant::now();
    let Request {
        task_id,
        id,
        program,
        dir,
        prompt,
        route,
        base_commit,
        dirty_at_start,
        before_run,
        recordings,
        worktree,
        classified,
        attachments,
        mut resume,
        continued,
    } = request;
    // Registered before the CLI is even spawned: a run is stoppable from the
    // moment the user can see it, including while a slow Node shim starts up.
    let mut control = live.open(task_id);
    let mut ctx = Context {
        task_id,
        id,
        program,
        dir,
        plan: StagePlan {
            stage: Stage::Implement,
            schema: None,
            tier: route.budget.preferred_tier,
            model: None,
            effort: None,
            shell: true,
        },
        final_stage: true,
        attachments,
    };
    let mut state = State {
        outcome: Outcome::default(),
        recording: Recording::new(None, task_id, Stage::Implement, id),
        calls_used: 0,
        turns_used: 0,
        constraints: Vec::new(),
        notes: Vec::new(),
        structured: None,
        budget_stop: None,
        halt: false,
        work_session: None,
        fix_changed: None,
        thread_totals: HashMap::new(),
        noise: Vec::new(),
        unknown_events: 0,
        held: Vec::new(),
        session: None,
        apply_now_pending: false,
        resume_point: None,
    };

    // The whole decision, recorded before a single process starts. Without this
    // row a later milestone can see what a run cost but not what it was allowed
    // to cost, and cannot tell a good route from a lucky one.
    if continued {
        let turn = serde_json::json!({ "kind": "turn", "data": { "prompt": prompt } });
        let _ = note(store, &ctx, "turn", &turn.to_string());
    }
    if let Err(e) = note(store, &ctx, "routing", &routing_payload(&route)) {
        state.outcome.failure = Some(format!("could not record the route: {}", e.message));
    }

    // The call that read the prompt is part of what this run cost.
    if !classified.is_empty() {
        let mut classified = classified;
        state.calls_used += 1;
        if id == ProviderId::Codex {
            price_codex(store, crate::intent::model(id), &mut state.outcome, &mut classified);
        }
        for event in &classified {
            match event {
                ProviderEvent::Usage(_) => state.outcome.absorb(event),
                ProviderEvent::Done { turns, .. } => state.turns_used += turns,
                _ => {}
            }
            let _ = record(store, task_id, "classify", id, event);
        }
        state.outcome.begin_process();
    }

    // The checks once before anything changes. A suite that already fails is
    // the task itself or a setup problem, such as a database that is not
    // running, and a failure afterwards cannot say which. So it buys no
    // automatic Fix, and the agent is shown what failed.
    let mut failing_before = Vec::new();
    if route.stages.contains(&Stage::Verify) && state.outcome.failure.is_none() {
        ctx.plan.stage = Stage::Verify;
        let scope = VerifyScope {
            index: 0,
            of: route.stages.len(),
            base_commit: base_commit.as_deref(),
            before_run: before_run.as_ref(),
            before_change: Some(&route.candidate_paths),
        };
        if verify_locally(store, &ctx, &mut state, &mut control, &emit, scope).await == Some(false) {
            failing_before = state.structured.as_ref().map(failed_checks).unwrap_or_default();
        }
        state.structured = None;
    }

    // Mutable because a failed check is followed by a Fix and the same check.
    // An independent Review runs at most once; its fix is judged by Verify.
    let mut stages = route.stages.clone();
    let mut index = 0;
    while index < stages.len() {
        let stage = stages[index];
        if state.outcome.failure.is_some()
            || state.outcome.cancelled
            || state.halt
            || state.budget_stop.is_some()
        {
            break;
        }
        // A Fix stays on the tier that wrote the change: a resumed session on
        // another model would re-read its whole history uncached.
        let tier = match (stage, route.budget.review_tier) {
            (Stage::Review, Some(review)) => review,
            _ => route.budget.preferred_tier,
        };
        let choice = match stage {
            Stage::Plan => route.plan_model(id),
            Stage::Review => route.review_model(id),
            Stage::Implement | Stage::Fix => route.work_model(id),
            _ => None,
        };
        // A Claude Fix resumes the Implement session, and any change to the tool
        // list re-bills that whole history uncached (~74k tokens on a real run).
        // `--json-schema` adds a StructuredOutput tool Implement never had, so a
        // Fix the next Verify judges goes without it.
        let schema = write_schema(task_id, stage).filter(|_| {
            !(id == ProviderId::Claude
                && stage == Stage::Fix
                && stages.get(index + 1) == Some(&Stage::Verify))
        });
        ctx.plan = StagePlan {
            stage,
            schema,
            tier,
            model: choice.map(|choice| choice.model),
            effort: choice.map(|choice| choice.effort),
            // A question may only run `git diff`/`status` anyway; the two shell
            // tool schemas cost more than that is worth on every question.
            // Implement keeps them so a resumed Fix sees the same tool list.
            shell: stage != Stage::Answer,
        };
        ctx.final_stage = index + 1 == stages.len();
        state.outcome.begin_stage();
        state.structured = None;
        state.session = None;
        state.recording = Recording::new(recordings.as_deref(), task_id, stage, id);
        // What a Fix is measured against: one that leaves this unchanged made
        // no progress, and another round would only repeat it.
        let before_fix = (stage == Stage::Fix)
            .then(|| project::patch_since(&ctx.dir, base_commit.as_deref()).ok())
            .flatten();

        // A Verify is a test command and a pass or a fail. When the repository
        // names its command Orteca runs it: no model call, no false failure
        // from an agent's shell, and a failure still buys the Fix call.
        let checked_locally = stage == Stage::Verify
            && verify_locally(
                store,
                &ctx,
                &mut state,
                &mut control,
                &emit,
                VerifyScope {
                    index,
                    of: stages.len(),
                    base_commit: base_commit.as_deref(),
                    before_run: before_run.as_ref(),
                    before_change: None,
                },
            )
            .await
            .is_some();
        if !checked_locally {
            // A follow-up resumes on the first stage that calls a model, and only
            // on the model it ran on: any other reads the whole history uncached.
            let resuming = resume
                .take()
                .filter(|r| r.model == ctx.plan.model(id).model);
            let words = resuming.as_ref().map_or(prompt.as_str(), |r| r.reply.as_str());
            let mut brief =
                routing::brief(&route, stage, words, &state.constraints, &state.notes);
            if id == ProviderId::Codex && stage == Stage::Implement && resuming.is_none() {
                brief.push_str(&pasted_files(&ctx.dir, &route.candidate_paths));
            }
            brief.push_str(&attached_note(&ctx.attachments));
            if stage == Stage::Review {
                brief.push_str(&pasted_diff(&ctx.dir, base_commit.as_deref()));
            }
            if stage == Stage::Implement && !failing_before.is_empty() {
                let output: Vec<String> = failing_before
                    .iter()
                    .map(|check| format!("{}
{}", check["command"].as_str().unwrap_or_default(), check["output"].as_str().unwrap_or_default()))
                    .collect();
                brief.push_str(&format!(
                    "

Before any change, the project's checks already fail. This may be the task, or a setup problem no edit can fix:
{}",
                    output.join("

")
                ));
            }
            if let Err(e) = note(
                store,
                &ctx,
                "stage",
                &stage_payload(stage, index, stages.len(), &ctx.plan, id),
            ) {
                state.outcome.failure = Some(format!("could not record the stage: {}", e.message));
                break;
            }

            // Usually one pass. A checkpoint provider told to apply an instruction
            // now ends its process and comes back through here resuming its own
            // session.
            let mut launch = match (&resuming, state.work_session.as_deref()) {
                (Some(r), _) => Launch::fix(id, &r.session, &brief, &ctx.plan),
                (None, Some(session)) if stage == Stage::Fix => {
                    Launch::fix(id, session, &brief, &ctx.plan)
                }
                _ => Launch::first(id, &brief, &ctx.plan),
            };
            loop {
                match attempt(store, &ctx, &mut state, &mut control, &emit, launch).await {
                    Next::Ended => break,
                    Next::Restart(again) => launch = again,
                }
            }
            if stage.writes() && state.session.is_some() {
                state.work_session = state.session.clone();
            }
            if let Some(session) = state.session.clone().filter(|_| stage.writes() || stage == Stage::Answer) {
                state.resume_point = Some(Resume {
                    session,
                    model: ctx.plan.model(id).model.into(),
                    ended_at: now_ms(),
                    reply: String::new(),
                });
            }
        }

        // A stage that ended in an instruction still waiting is not a stage
        // that lost it: the constraint list carries it into the next brief.
        state.held.clear();
        if stage == Stage::Fix {
            let after = project::patch_since(&ctx.dir, base_commit.as_deref()).ok();
            state.fix_changed =
                Some(before_fix.is_none() || after.is_none() || before_fix != after);
        }
        let fixing_review = stage == Stage::Fix
            && state
                .notes
                .last()
                .is_some_and(|note| note.stage == Stage::Review);
        let (mut note_for_stage, failed) = finish_stage(store, &ctx, &mut state, stage);
        if checked_locally {
            // Orteca ran the tests itself; no model was asked anything.
            (note_for_stage.model, note_for_stage.effort) = (None, None);
            // No model spoke, so the summary would fall back to an earlier
            // stage - a Fix's own "pass" over the Verify that just failed.
            if failed {
                note_for_stage.summary = failed_summary(note_for_stage.artifact.as_ref());
            }
        }
        state.notes.push(note_for_stage);
        // An Implement that changed nothing has nothing to check. Testing an
        // untouched tree only makes the user wait for the same answer.
        if stage == Stage::Implement
            && state.outcome.failure.is_none()
            && !state.outcome.cancelled
            && changed_nothing(&ctx.dir, base_commit.as_deref(), before_run.as_ref())
        {
            break;
        }
        let round = Round {
            stage,
            // A Fix that a Verify follows is judged by that Verify.
            failed: failed
                && state.outcome.failure.is_none()
                && !(stage == Stage::Fix && stages.get(index + 1) == Some(&Stage::Verify)),
            fixing_review,
            fix_changed: state.fix_changed,
            notes: &state.notes,
            failing_before: &failing_before,
            verifies_before_review: route.verifies_before_review(),
            remaining: &stages[index + 1..],
        };
        match after_stage(round) {
            Then::Continue => {}
            Then::Stop(stop) => {
                let _ = note(store, &ctx, "budget", &budget_payload("stopped", &stop));
                state.halt = true;
                state.budget_stop = Some(stop);
            }
            Then::Retry(again) => {
                stages.splice(index + 1..index + 1, again);
            }
        }
        index += 1;
    }

    let dir = ctx.dir;
    let mut outcome = state.outcome;
    outcome.settle_cost();

    live.close(task_id);

    // A run that never reported usage has no honest number; this writes the
    // `unavailable` row rather than leaving the task looking free.
    let recorded = if continued {
        store.add_usage(task_id, id.program(), outcome.usage.as_ref())
    } else {
        store.record_usage(task_id, None, id.program(), outcome.usage.as_ref())
    };
    if let Err(e) = recorded {
        outcome.failure = Some(format!("could not save usage: {}", e.message));
    }

    let diff = match project::diff_since(&dir, base_commit.as_deref()) {
        Ok(mut diff) => {
            project::attribute(&dir, &mut diff, before_run.as_ref());
            diff
        }
        Err(e) => {
            outcome.failure.get_or_insert(e.message);
            Vec::new()
        }
    };
    let patch_text = match project::patch_since(&dir, base_commit.as_deref()) {
        Ok(patch) if patch.is_empty() => None,
        Ok(patch) => Some(patch),
        Err(e) => {
            outcome.failure.get_or_insert(e.message);
            None
        }
    };
    // Committed whatever the outcome: a stopped run's work is still work, and on
    // its own branch it is one merge away instead of loose in a folder.
    let worktree = worktree.map(|mut copy| {
        if !diff.is_empty() {
            match project::commit_worktree(&dir, &commit_message(task_id, &prompt)) {
                Ok(sha) => copy.commit = Some(sha),
                Err(e) => copy.commit_error = Some(e.message),
            }
        }
        copy
    });
    // Codex on Windows can run `workspace-write` as read-only and still end its
    // turn cleanly. Every command is rejected inside the CLI, and `--json`
    // carries none of it - the rejections exist only in Codex's own rollout.
    // What the stream cannot hide is the repository: a writing stage that left
    // no change of its own did not do what `done` would claim. Prose is not read.
    // Claude gets the same check: it can answer an implement brief with a
    // question and change nothing, and that is not a finished task either.
    if outcome.failure.is_none()
        && !outcome.cancelled
        && state.budget_stop.is_none()
        && state.notes.iter().any(|n| n.stage.writes())
        && ran_nothing(&diff)
    {
        outcome.failure = Some(match id {
            ProviderId::Codex => "Codex ended its implement stage without changing any file. A sandbox that fell back to read-only looks exactly like this.".into(),
            _ => format!("{} ended its implement stage without changing any file. Its reply is above.", id.program()),
        });
    }
    let duration_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if let (Some(message), Some(kind)) = (&outcome.failure, outcome.failure_kind()) {
        let event = ProviderEvent::Failed {
            kind,
            message: message.clone(),
        };
        if let Err(e) = record(store, task_id, "run", id, &event) {
            outcome.failure = Some(format!("{message}; could not log failure: {}", e.message));
        }
    }
    // A budget stop is its own outcome. It is not a failure - nothing went
    // wrong - and not a success, because the route did not finish. The work,
    // the diff and the usage are all kept exactly as they are.
    let mut status =
        if state.budget_stop.is_some() && outcome.failure.is_none() && !outcome.cancelled {
            match state.budget_stop.as_ref().map(|stop| stop.limit) {
                Some("review") => "reviewRejected",
                Some("verify") => "verifyFailed",
                _ => "budgetReached",
            }
        } else {
            outcome.status()
        };
    // The last stage that actually said something. A Review that asked for
    // changes is the answer to the task, not the Verify that never ran. A
    // finished route skips the checkers' JSON: the writer's words are the answer.
    let summary = state
        .notes
        .iter()
        .rev()
        .map(|n| n.summary.clone())
        .find(|s| {
            !s.trim().is_empty()
                && !(status == "done"
                    && serde_json::from_str::<serde_json::Value>(s).is_ok_and(|v| v.is_object()))
        })
        .unwrap_or_else(|| outcome.summary());
    // Read before this task closes, so it is never its own comparison. A
    // baseline that cannot be read is no baseline, not a failed run.
    let baseline = serde_json::to_value(route.kind).ok().and_then(|kind| {
        store
            .baseline(task_id, kind.as_str()?, id.program())
            .ok()
            .flatten()
    });
    if let Err(e) = store.finish_task_details(
        task_id,
        status,
        &summary,
        &serde_json::to_string(&diff).unwrap_or_else(|_| "[]".into()),
        patch_text.as_deref(),
        state.unknown_events,
        Some(duration_ms),
        state.calls_used,
    ) {
        status = "failed";
        outcome.failure = Some(format!("could not finish task record: {}", e.message));
    }

    TaskResult {
        task_id,
        status,
        summary,
        failure_kind: outcome.failure_kind(),
        failure: outcome.failure,
        usage: outcome.usage,
        diff,
        patch_text,
        unknown_events: state.unknown_events,
        duration_ms,
        dirty_at_start,
        route,
        stages: state.notes,
        calls_used: state.calls_used,
        turns_used: state.turns_used,
        budget_stop: state.budget_stop,
        baseline,
        resume: state.resume_point.filter(|_| worktree.is_none()),
        worktree,
    }
}

/// The repository declares a test command and it is on PATH, so a Verify costs
/// no agent call.
pub fn checks_locally(dir: &Path) -> bool {
    project::check_commands(dir).iter().any(runnable)
}

/// Every program a suite needs, its install included, is on PATH.
fn runnable(check: &project::Check) -> bool {
    check
        .install
        .iter()
        .chain(std::iter::once(&check.test))
        .all(|command| crate::providers::which(command[0]).is_some())
}

/// Small candidate files pasted into a Codex Implement brief, so Codex spends no
/// shell call opening them. Claude gets none: its Edit tool refuses a file it has
/// not Read in the same session, so pasting would pay for the bytes twice.
/// Symlinks are skipped, because a tracked link can point outside the repository.
// ponytail: first three candidates, 8 KB each, 16 KB in all; tune once runs show
// how often a pasted file was the one edited.
/// The change a Review reads, pasted so it does not spend turns finding it.
/// A patch too large to paste is left for the reviewer to open.
fn pasted_diff(dir: &Path, base: Option<&str>) -> String {
    const MAX_BYTES: usize = 48 * 1024;
    match project::patch_since(dir, base) {
        Ok(patch) if !patch.trim().is_empty() && patch.len() <= MAX_BYTES => format!(
            "\nThe change under review, as a patch against where the run started. Open \
             other files only where the patch is not enough:\n```diff\n{}\n```\n",
            patch.trim_end()
        ),
        _ => String::new(),
    }
}

fn pasted_files(dir: &Path, paths: &[String]) -> String {
    const EACH: u64 = 8_000;
    const TOTAL: usize = 16_000;
    let mut out = String::new();
    for path in paths.iter().take(3) {
        let full = dir.join(path);
        let Ok(meta) = std::fs::symlink_metadata(&full) else {
            continue;
        };
        if !meta.is_file() || meta.len() > EACH {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&full) else {
            continue;
        };
        if out.len() + text.len() > TOTAL {
            continue;
        }
        out.push_str(&format!("\n--- {path} ---\n{text}\n"));
    }
    if out.is_empty() {
        return out;
    }
    format!("\nCurrent contents of the likeliest paths above, so they need no opening:\n{out}--- end ---\n")
}

/// Run the repository's own test suites as the Verify stage, with no model.
///
/// False, with nothing counted, when no suite is declared or none can start:
/// the stage then goes to the agent as before. A suite whose dependencies are
/// missing has them installed first. Suites run as the user on either provider,
/// as the project's trust consent covers: inside Codex's Windows sandbox a Vite,
/// Jest or npm suite cannot read the folders above a repository under the user
/// profile and dies before it tests anything, and an install needs the network
/// the sandbox denies. A failure that is the fence's buys a Fix no edit can win.
struct VerifyScope<'a> {
    index: usize,
    of: usize,
    base_commit: Option<&'a str>,
    before_run: Option<&'a project::Snapshot>,
    /// Set for the check before any change: the paths the route expects to
    /// touch pick the suites, since nothing has changed yet.
    before_change: Option<&'a [String]>,
}

/// Trees whose checks passed before a run, keyed by folder, commit, what was
/// already dirty and which suites ran. A pass is only as old as this launch.
// ponytail: in memory, so a restart runs the suites again; a table if that hurts.
static PASSED_BEFORE: Mutex<Vec<u64>> = Mutex::new(Vec::new());

/// Returns `None` when Orteca cannot run the checks itself, otherwise whether
/// they passed. A cancelled check is not a pass.
async fn verify_locally(
    store: &Store,
    ctx: &Context,
    state: &mut State,
    control: &mut mpsc::UnboundedReceiver<Control>,
    emit: &impl Fn(&ProviderEvent) -> crate::error::Result<()>,
    scope: VerifyScope<'_>,
) -> Option<bool> {
    let changed_paths: Vec<String> = match scope.before_change {
        Some(expected) => expected.to_vec(),
        None => {
            let mut changed =
                project::diff_since(&ctx.dir, scope.base_commit).unwrap_or_default();
            project::attribute(&ctx.dir, &mut changed, scope.before_run);
            changed
                .into_iter()
                .filter(|file| file.origin != Some(project::Origin::BeforeRun))
                .map(|file| file.path)
                .collect()
        }
    };
    let (mut checks, missing): (Vec<_>, Vec<_>) =
        project::relevant_check_commands(&ctx.dir, &changed_paths)
        .into_iter()
        .partition(runnable);
    let before_change = scope.before_change.is_some();
    if before_change {
        checks.retain(|check| quick_check(check).is_some());
    }
    if checks.is_empty() {
        return None;
    }
    let label = |check: &project::Check, command: &[&str]| match check.dir.strip_prefix(&ctx.dir) {
        Ok(sub) if !sub.as_os_str().is_empty() => format!(
            "{}: {}",
            sub.to_string_lossy().replace('\\', "/"),
            command.join(" ")
        ),
        _ => command.join(" "),
    };
    let shown: Vec<String> = checks.iter().map(|c| label(c, &c.test)).collect();

    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        (&ctx.dir, scope.base_commit, scope.before_run, &shown).hash(&mut hasher);
        hasher.finish()
    };
    if before_change {
        if PASSED_BEFORE.lock().is_ok_and(|passed| passed.contains(&key)) {
            return Some(true);
        }
        let event = ProviderEvent::Text("Running the checks once before anything changes".into());
        let _ = record(store, ctx.task_id, ctx.stage(), ctx.id, &event).and_then(|()| emit(&event));
    } else {
        let stage = serde_json::json!({
            "kind": "stage",
            "data": { "stage": Stage::Verify, "index": scope.index, "of": scope.of, "writes": false, "schema": true, "runner": "orteca", "command": shown.join(", ") },
        });
        let _ = note(store, ctx, "stage", &stage.to_string());
    }
    // A suite with nothing on PATH to run it is said out loud, never passed.
    for check in missing.iter().filter(|_| !before_change) {
        let event = ProviderEvent::Text(format!(
            "{} did not run: it is not on PATH",
            label(check, &check.test)
        ));
        let _ = record(store, ctx.task_id, ctx.stage(), ctx.id, &event).and_then(|()| emit(&event));
    }

    let mut results = Vec::new();
    for (check, shown) in checks.iter().zip(shown) {
        let mut ran = Ran::Finished {
            passed: true,
            output: String::new(),
        };
        if let Some(install) = &check.install {
            let named = label(check, install);
            ran = run_check(store, ctx, state, control, emit, install, &check.dir, &named).await;
        }
        if matches!(ran, Ran::Finished { passed: true, .. }) && !before_change {
            if let Some(focused) = focused_php_check(&ctx.dir, check, &changed_paths) {
                let args: Vec<&str> = focused.iter().map(String::as_str).collect();
                let named = label(check, &args);
                ran = run_check(store, ctx, state, control, emit, &args, &check.dir, &named).await;
                if let Ran::Finished {
                    passed: true,
                    output,
                } = &ran
                {
                    results.push(serde_json::json!({
                        "command": named,
                        "passed": true,
                        "output": output,
                    }));
                }
            }
        }
        // Before the change the quick check stands in for the suite, and its
        // result is recorded under the suite's name so a later Verify matches it.
        if matches!(ran, Ran::Finished { passed: true, .. }) {
            ran = match quick_check(check).filter(|_| before_change) {
                Some(quick) => {
                    let args: Vec<&str> = quick.iter().map(String::as_str).collect();
                    let named = label(check, &args);
                    run_check(store, ctx, state, control, emit, &args, &check.dir, &named).await
                }
                None => {
                    run_check(store, ctx, state, control, emit, &check.test, &check.dir, &shown)
                        .await
                }
            };
        }
        match ran {
            Ran::Cancelled => return Some(false),
            Ran::NotStarted => {}
            // A failed install is reported under its suite, with what it printed.
            Ran::Finished { passed, output } => {
                if !passed
                    && !before_change
                    && missing_program(&output)
                    && state.outcome.failure.is_none()
                {
                    state.outcome.failure = Some(format!(
                        "{shown} could not run because a required program is missing. No automatic fix was started."
                    ));
                }
                results.push(
                    serde_json::json!({ "command": shown, "passed": passed, "output": output }),
                );
            }
        }
    }
    if results.is_empty() {
        return None;
    }
    let passed = results.iter().all(|check| check["passed"] == true);
    // A tree git could not list has no fingerprint to remember a pass by.
    if before_change && passed && scope.before_run.is_some() {
        if let Ok(mut passed_before) = PASSED_BEFORE.lock() {
            passed_before.push(key);
        }
    }
    state.structured = Some(serde_json::json!({
        "checks": results,
        "verdict": if passed { "pass" } else { "fail" },
    }));
    Some(passed)
}

/// A changed Laravel test is the cheapest useful tripwire before a broad
/// Composer suite. Other ecosystems keep their declared command unchanged.
fn focused_php_check(root: &Path, check: &project::Check, changed: &[String]) -> Option<Vec<String>> {
    if check.kind != "php" || !check.dir.join("artisan").is_file() {
        return None;
    }
    let prefix = check
        .dir
        .strip_prefix(root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let mut tests: Vec<String> = changed
        .iter()
        .filter_map(|path| {
            let relative = if prefix.is_empty() {
                path.as_str()
            } else {
                path.strip_prefix(&format!("{prefix}/"))?
            };
            (relative.starts_with("tests/") && relative.ends_with(".php"))
                .then(|| relative.replace('\\', "/"))
        })
        .take(8)
        .collect();
    if tests.is_empty() {
        return None;
    }
    let mut command = vec!["php".into(), "artisan".into(), "test".into()];
    command.append(&mut tests);
    Some(command)
}

/// One small test file for the check before a change: it boots the app and,
/// in a Laravel feature test, its database, so a broken setup shows in seconds
/// instead of a full suite. A single file also keeps a flaky parallel test from
/// marking the whole suite as failing. Other ecosystems have no generic small
/// slice, so they skip the check before the change.
// ponytail: Laravel only, smallest file by size; add an ecosystem when one has a cheap slice.
fn quick_check(check: &project::Check) -> Option<Vec<String>> {
    if check.kind != "php" || !check.dir.join("artisan").is_file() {
        return None;
    }
    let file = ["tests/Feature", "tests/Unit"].iter().find_map(|sub| {
        std::fs::read_dir(check.dir.join(sub))
            .ok()?
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_file()))
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                name.ends_with("Test.php")
                    .then_some((entry.metadata().ok()?.len(), name))
            })
            .min()
            .map(|(_, name)| format!("{sub}/{name}"))
    })?;
    Some(vec!["php".into(), "artisan".into(), "test".into(), file])
}

fn missing_program(output: &str) -> bool {
    let output = output.to_ascii_lowercase();
    output.contains("is not recognized as an internal or external command")
        || output.contains("command not found")
}

enum Ran {
    Finished { passed: bool, output: String },
    NotStarted,
    Cancelled,
}

/// One command to its end, streamed into the run as Orteca's own tool use.
#[allow(clippy::too_many_arguments)]
async fn run_check(
    store: &Store,
    ctx: &Context,
    state: &mut State,
    control: &mut mpsc::UnboundedReceiver<Control>,
    emit: &impl Fn(&ProviderEvent) -> crate::error::Result<()>,
    command: &[&str],
    cwd: &Path,
    shown: &str,
) -> Ran {
    // ponytail: ten minutes per command is a guess at a slow suite; make it per project when one needs longer.
    const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
    let say = |event: ProviderEvent| {
        let _ = record(store, ctx.task_id, ctx.stage(), ctx.id, &event).and_then(|()| emit(&event));
    };
    let Some(program) = crate::providers::which(command[0]) else {
        return Ran::NotStarted;
    };
    let Ok(mut run) = proc::spawn(&program.to_string_lossy(), &command[1..], cwd) else {
        say(ProviderEvent::Text(format!("{shown} did not start")));
        return Ran::NotStarted;
    };
    say(ProviderEvent::ToolUse {
        name: "orteca".into(),
        summary: shown.to_string(),
    });

    let mut tail = std::collections::VecDeque::new();
    let mut code = None;
    let mut timed_out = false;
    let deadline = tokio::time::sleep(TIMEOUT);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            line = run.lines.recv() => match line {
                Some(Line::Exit(exit)) => { code = exit; break; }
                Some(Line::Text(text)) => tail.push_back(text),
                Some(Line::Json(value)) => tail.push_back(value.to_string()),
                None => break,
            },
            Some(action) = control.recv() => match action {
                Control::Cancel => { answer(Control::Cancel, store, ctx, state, &mut run).await; }
                // No agent is running to take it; the next brief carries it, as
                // it carries any instruction that arrived between stages.
                Control::Instruct { text, reply, .. } => {
                    state.constraints.push(text.clone());
                    let _ = note(store, ctx, "instruction", &instruction(&text, InstructionDisposition::Held));
                    let _ = reply.send(InstructionReceipt { disposition: InstructionDisposition::Held });
                }
            },
            () = &mut deadline, if !timed_out => { timed_out = true; run.cancel(); }
        }
        // The end of a suite's output is where it says what failed.
        while tail.len() > 60 {
            tail.pop_front();
        }
    }
    if state.outcome.cancelled {
        return Ran::Cancelled;
    }
    let mut output = Vec::from(tail).join("\n");
    if timed_out {
        output.push_str("\nStopped by Orteca after 10 minutes.");
    }
    let passed = code == Some(0) && !timed_out;
    say(ProviderEvent::Text(format!(
        "{shown} {}",
        if passed { "passed" } else { "did not pass" }
    )));
    Ran::Finished { passed, output }
}

fn commit_message(task_id: i64, prompt: &str) -> String {
    let line: String = prompt
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect();
    format!("Orteca task {task_id}: {line}")
}

/// A finished stage, as far as deciding what follows it.
struct Round<'a> {
    stage: Stage,
    /// A check did not pass, nothing failed around it, and no later stage
    /// already judges it.
    failed: bool,
    /// This stage is a Fix for a Review's findings.
    fixing_review: bool,
    fix_changed: Option<bool>,
    notes: &'a [StageNote],
    failing_before: &'a [serde_json::Value],
    verifies_before_review: bool,
    remaining: &'a [Stage],
}

enum Then {
    Continue,
    Stop(BudgetStop),
    /// Run these stages next.
    Retry(Vec<Stage>),
}

/// What follows a finished stage. A Fix that changed nothing stops the run; a
/// suite that failed before the change, or a Verify that already had its Fix,
/// hands back to the user; anything else that failed gets a Fix and its check.
fn after_stage(round: Round<'_>) -> Then {
    let remaining = || round.remaining.to_vec();
    if round.fixing_review && round.fix_changed == Some(false) {
        return Then::Stop(stuck(round.notes, remaining()));
    }
    if !round.failed {
        return Then::Continue;
    }
    if round.fix_changed == Some(false) {
        return Then::Stop(stuck(round.notes, remaining()));
    }
    if round.stage == Stage::Verify {
        let failed_again = round
            .notes
            .last()
            .and_then(|note| note.artifact.as_ref())
            .is_some_and(|artifact| {
                failed_checks(artifact).iter().any(|now| {
                    round
                        .failing_before
                        .iter()
                        .any(|then| then["command"] == now["command"])
                })
            });
        if failed_again {
            return Then::Stop(failed_before_run(remaining()));
        }
        if round.notes.iter().any(|note| note.stage == Stage::Fix) {
            return Then::Stop(fix_limit(remaining()));
        }
        return Then::Retry(vec![Stage::Fix, Stage::Verify]);
    }
    // A Review runs once. Its fix is judged by deterministic Verify, whether
    // Verify was originally before or after it.
    if round.stage == Stage::Review && round.verifies_before_review {
        return Then::Retry(vec![Stage::Fix, Stage::Verify]);
    }
    Then::Retry(vec![Stage::Fix])
}

/// The stop a Fix that changed nothing leaves behind: the check it answered
/// still does not pass, and another round would only repeat it.
fn stuck(notes: &[StageNote], remaining: Vec<Stage>) -> BudgetStop {
    let review = notes
        .iter()
        .rev()
        .find(|n| matches!(n.stage, Stage::Review | Stage::Verify))
        .is_some_and(|n| n.stage == Stage::Review);
    BudgetStop {
        limit: if review { "review" } else { "verify" },
        allowed: 0,
        observed: 0,
        remaining,
        message: if review {
            "The last fix changed nothing and the review still asks for changes. Its findings are kept; what to do next is up to you."
        } else {
            "The last fix changed nothing and the checks still do not pass. What they printed is kept; what to do next is up to you."
        }
        .into(),
    }
}

/// A failed Verify gets one automatic repair. More attempts can consume an
/// allowance without converging, so the failing output is returned to the user.
fn fix_limit(remaining: Vec<Stage>) -> BudgetStop {
    BudgetStop {
        limit: "verify",
        allowed: 1,
        observed: 1,
        remaining,
        message: "One automatic fix was attempted and the checks still do not pass. Their output is kept; another attempt is the user's decision."
            .into(),
    }
}

/// A check that failed before the run changed anything fails again. Orteca
/// cannot tell a broken setup from unfinished work, so the user decides.
fn failed_before_run(remaining: Vec<Stage>) -> BudgetStop {
    BudgetStop {
        limit: "verify",
        allowed: 0,
        observed: 0,
        remaining,
        message: "These checks already failed before the run changed anything, so no automatic fix was started. Their output is kept; check the project's setup, then run again."
            .into(),
    }
}

/// Every check in a Verify artifact that did not pass.
fn failed_checks(artifact: &serde_json::Value) -> Vec<serde_json::Value> {
    artifact["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|check| check["passed"] != true)
        .cloned()
        .collect()
}

/// A local Verify that failed, in words: each failing command and its output.
fn failed_summary(artifact: Option<&serde_json::Value>) -> String {
    artifact
        .map(failed_checks)
        .unwrap_or_default()
        .iter()
        .map(|check| {
            format!(
                "`{}` did not pass.\n\n```\n{}\n```",
                check["command"].as_str().unwrap_or("check"),
                check["output"].as_str().unwrap_or_default().trim()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Close out one stage: validate whatever artifact came back, log it, and say
/// whether a check it contracted for did not pass.
/// Every file in the diff was already changed before the run started.
fn ran_nothing(diff: &[project::FileStat]) -> bool {
    diff.iter().all(|f| f.origin == Some(project::Origin::BeforeRun))
}

/// The run has not changed the repository yet. A diff that cannot be read
/// counts as a change: stopping early on a guess would hide real work.
fn changed_nothing(dir: &Path, base: Option<&str>, before: Option<&project::Snapshot>) -> bool {
    project::diff_since(dir, base).is_ok_and(|mut diff| {
        project::attribute(dir, &mut diff, before);
        ran_nothing(&diff)
    })
}

fn finish_stage(store: &Store, ctx: &Context, state: &mut State, stage: Stage) -> (StageNote, bool) {
    // An artifact counts only if it arrived as structured output and matches
    // the shape the stage contracted for. The alternative - reading the closing
    // prose and filling the fields from it - is exactly the guesswork the
    // schema exists to remove, so a missing artifact stays missing.
    let artifact = state
        .structured
        .take()
        .filter(|value| routing::artifact_is_valid(stage, value));
    if ctx.plan.schema.is_some() {
        let _ = note(
            store,
            ctx,
            "artifact",
            &artifact_payload(stage, artifact.as_ref()),
        );
    }
    // A review that asks for changes, a missing artifact, or checks that did
    // not pass: the caller follows it with a Fix. A passing check clears the
    // no-progress record, so an old Fix never stops a new round.
    let failed = matches!(stage, Stage::Review | Stage::Verify | Stage::Fix)
        && !artifact
            .as_ref()
            .is_some_and(|a| routing::stage_passed(stage, a));
    if !failed && matches!(stage, Stage::Review | Stage::Verify) {
        state.fix_changed = None;
    }
    (
        StageNote {
            stage,
            summary: state.outcome.summary(),
            artifact,
            model: Some(ctx.plan.model(ctx.id).model.to_string()),
            effort: Some(ctx.plan.model(ctx.id).effort.to_string()),
        },
        failed,
    )
}

/// The route, whole, as the event log stores it.
fn routing_payload(route: &Route) -> String {
    serde_json::json!({ "kind": "routing", "data": route }).to_string()
}

fn stage_payload(
    stage: Stage,
    index: usize,
    of: usize,
    plan: &StagePlan,
    id: ProviderId,
) -> String {
    let choice = plan.model(id);
    serde_json::json!({
        "kind": "stage",
        "data": {
            "stage": stage,
            "index": index,
            "of": of,
            "writes": stage.writes(),
            "schema": stage.schema().is_some(),
            // What Orteca asked for. Codex names no model in its events, so
            // for Codex this is the only record of it.
            "model": choice.model,
            "effort": choice.effort,
            "shell": plan.shell,
        }
    })
    .to_string()
}

fn budget_payload(decision: &str, stop: &BudgetStop) -> String {
    serde_json::json!({
        "kind": "budget",
        "data": {
            "decision": decision,
            "limit": stop.limit,
            "allowed": stop.allowed,
            "observed": stop.observed,
            "remaining": stop.remaining,
        }
    })
    .to_string()
}

/// An artifact row is written whether or not one arrived. A stage that
/// contracted for a shape and returned nothing is a fact worth keeping.
fn artifact_payload(stage: Stage, artifact: Option<&serde_json::Value>) -> String {
    serde_json::json!({
        "kind": "artifact",
        "data": { "stage": stage, "valid": artifact.is_some(), "artifact": artifact }
    })
    .to_string()
}

/// Codex reports tokens only. Each turn is priced at the published rate of the
/// model this stage asked for, as a running total for the process - the shape
/// `Usage::absorb` and `begin_process` already give a cost. One unpriced turn
/// voids the task's cost: a total missing a turn reads cheaper than it was.
/// Codex reports a thread's running totals, and a resumed process carries on
/// the same thread: checked against its rollout on 2026-09-14, a Fix resumed
/// after a 418k Implement reported 1.15M. Each report is turned into what it
/// added since the thread's previous one, or a resumed Fix bills the Implement
/// again.
fn added_since(
    totals: &mut HashMap<String, Usage>,
    session: Option<&str>,
    events: &mut [ProviderEvent],
) {
    let Some(session) = session else {
        return;
    };
    for event in events {
        let ProviderEvent::Usage(usage) = event else {
            continue;
        };
        let reported = usage.clone();
        if let Some(before) = totals.get(session) {
            usage.input_tokens = usage.input_tokens.saturating_sub(before.input_tokens);
            usage.cached_input_tokens = usage
                .cached_input_tokens
                .saturating_sub(before.cached_input_tokens);
            usage.output_tokens = usage.output_tokens.saturating_sub(before.output_tokens);
            usage.reasoning_tokens = usage.reasoning_tokens.saturating_sub(before.reasoning_tokens);
        }
        totals.insert(session.to_string(), reported);
    }
}

fn price_codex(store: &Store, model: &str, outcome: &mut Outcome, events: &mut [ProviderEvent]) {
    for event in events {
        let ProviderEvent::Usage(usage) = event else {
            continue;
        };
        // Codex names no model, so the result would read "unknown" without this.
        usage.model.get_or_insert_with(|| model.to_string());
        match store.price(model) {
            Some(price) => {
                let so_far = outcome.usage.as_ref().and_then(|u| u.cost_usd).unwrap_or(0.0);
                usage.cost_usd = Some(so_far + codex::estimate(usage, price));
                usage.cost_quality = CostQuality::Estimated;
            }
            None => outcome.unpriced = true,
        }
    }
}

/// The user's attachments, as a line the agent can act on. Empty when none.
fn attached_note(attachments: &[PathBuf]) -> String {
    if attachments.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "

The user attached these files and folders on purpose. Attaching is the user sharing them with you, so open any attached image rather than asking what it shows. Read or view the rest as the task needs; do not edit them unless the task says to:
",
    );
    for path in attachments {
        out.push_str(&format!("- {}
", path.display()));
    }
    out
}

/// Codex reads outside the workspace under its sandbox already. Claude only
/// reaches directories it is given, so each attachment's folder is added.
// ponytail: --add-dir also lets acceptEdits write there; the brief asks it not to.
fn attachment_args(id: ProviderId, attachments: &[PathBuf]) -> Vec<String> {
    if id != ProviderId::Claude {
        return Vec::new();
    }
    // Paths, not strings: `C:\a\` and `C:\a` are one folder.
    let mut dirs: Vec<&Path> = attachments
        .iter()
        .filter_map(|p| if p.is_dir() { Some(p.as_path()) } else { p.parent() })
        .collect();
    dirs.sort();
    dirs.dedup();
    dirs.into_iter()
        .flat_map(|d| ["--add-dir".to_string(), d.display().to_string()])
        .collect()
}

/// Run one process to its end. Says whether the task is finished or is being
/// picked back up somewhere else.
async fn attempt(
    store: &Store,
    ctx: &Context,
    state: &mut State,
    control: &mut mpsc::UnboundedReceiver<Control>,
    emit: &impl Fn(&ProviderEvent) -> crate::error::Result<()>,
    launch: Launch,
) -> Next {
    let mut argv = launch.argv;
    argv.extend(attachment_args(ctx.id, &ctx.attachments));
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    // Counted before the spawn can fail: a process Orteca tried to start is a
    // call it spent, and hiding the failures would flatter the metric.
    state.calls_used = state.calls_used.saturating_add(1);
    state.outcome.begin_process();
    let mut run = match proc::spawn(&ctx.program.to_string_lossy(), &borrowed, &ctx.dir) {
        Ok(run) => run,
        Err(e) => {
            state.outcome.failure = Some(format!("could not start {}: {e}", ctx.id.program()));
            return Next::Ended;
        }
    };
    // Prompt bytes bypass cmd.exe parsing and its command-line limit.
    if let Err(e) = run.send_line(&launch.opening).await {
        state.outcome.failure = Some(format!(
            "could not send prompt to {}: {e}",
            ctx.id.program()
        ));
    }
    // A checkpoint provider is told no more input is coming. A live one keeps
    // its stdin, because that is what an instruction travels down - which also
    // means the run ends when Orteca closes stdin rather than when the provider
    // reports a result. `claude --input-format stream-json` sits waiting for
    // another turn indefinitely.
    if ctx.id.steering() == Steering::Checkpoint {
        run.close_stdin();
    }

    // Set once this process is known to be ending, so whatever it has already
    // written is still drained before the decision is acted on.
    let mut ending: Option<Next> = None;
    loop {
        let next = tokio::select! {
            line = run.lines.recv() => line,
            // Retired once this process is ending - a second stop has nothing
            // left to kill - and again if the sender is dropped, so a closed
            // control channel cannot spin this loop.
            Some(action) = control.recv(), if ending.is_none() => {
                ending = answer(action, store, ctx, state, &mut run).await;
                continue;
            }
        };
        let Some(line) = next else { break };
        match line {
            Line::Json(value) => {
                // Before parsing: the recording is what the CLI said, not what
                // Orteca understood of it.
                state.recording.write(&value);
                // Only stages with a schema may produce an artifact. This is
                // provider-specific machine output, not a best-effort parse of
                // ordinary closing prose.
                if ctx.plan.schema.is_some() {
                    if let Some(structured) = ctx.id.structured_output(&value) {
                        state.structured = Some(structured);
                    }
                }
                let mut events = ctx.id.parse_line(&value);
                if ctx.id == ProviderId::Codex {
                    added_since(&mut state.thread_totals, state.session.as_deref(), &mut events);
                    price_codex(store, ctx.plan.model(ctx.id).model, &mut state.outcome, &mut events);
                }
                // Each parser understands a subset of its CLI's event types and
                // silently drops the rest. Harmless for the stream, fatal for
                // the record: a Codex run whose tool calls all failed said so in
                // an item type this parser does not know, and the log kept no
                // trace of why the run did nothing. Keep the raw line. It is not
                // emitted - the UI has no shape for it - so the log stays
                // complete while the stream stays readable.
                // Claude's running thinking-token count says nothing the usage
                // event does not; logging it buried real unknowns 3 to 1.
                let tick = value["type"] == "system" && value["subtype"] == "thinking_tokens";
                if events.is_empty() && !tick {
                    state.unknown_events = state.unknown_events.saturating_add(1);
                    if let Err(e) = store.append_event(
                        ctx.task_id,
                        ctx.stage(),
                        "unknown",
                        ctx.id.program(),
                        &value.to_string(),
                    ) {
                        state.outcome.failure =
                            Some(format!("could not record run event: {}", e.message));
                        break;
                    }
                }
                for event in events {
                    if let Err(e) = record(store, ctx.task_id, ctx.stage(), ctx.id, &event)
                        .and_then(|()| emit(&event))
                    {
                        state.outcome.failure = Some(format!(
                            "could not record or deliver run event: {}",
                            e.message
                        ));
                        break;
                    }
                    if let ProviderEvent::Started { session_id } = &event {
                        state.session = Some(session_id.clone());
                    }
                    if let ProviderEvent::Done {
                        structured, turns, ..
                    } = &event
                    {
                        // Kept raw. Whether it is an artifact is decided by the
                        // stage's own contract once the stage is over.
                        if structured.is_some() {
                            state.structured = structured.clone();
                        }
                        state.turns_used = state.turns_used.saturating_add(*turns);
                    }
                    state.outcome.absorb(&event);
                    if matches!(event, ProviderEvent::Done { .. }) {
                        match ctx.id.steering() {
                            // The CLI ends itself once its one turn is done.
                            Steering::Checkpoint => state.outcome.finished = true,
                            Steering::Live => {
                                // A live instruction accepted before this
                                // result is incorporated into the same turn by
                                // current Claude Code. It does not create a
                                // second result, so this result completes every
                                // instruction the open stdin accepted.
                                state.outcome.finished = true;
                                run.close_stdin();
                            }
                        }
                    }
                }
                if state.outcome.failure.is_some() {
                    break;
                }
                if ending.is_none() && state.apply_now_pending {
                    ending = restart_held(ctx, state, &run);
                }
            }
            Line::Text(text) => {
                if !text.trim().is_empty() {
                    state.noise.push(text);
                    // Only the tail matters for a failure message.
                    if state.noise.len() > 5 {
                        state.noise.remove(0);
                    }
                }
            }
            Line::Exit(code) => {
                // A process ended on purpose - stopped by the user, or handed
                // over to a resume - has nothing to explain.
                if ending.is_none() {
                    state.outcome.exited(ctx.id, code, &state.noise);
                    // Only the last stage resumes itself to deliver a held
                    // instruction. Anywhere earlier there is a brief coming that
                    // will carry it, and an extra process to say it twice is
                    // exactly the waste this milestone exists to stop.
                    if state.outcome.failure.is_none()
                        && ctx.id.steering() == Steering::Checkpoint
                        && ctx.final_stage
                        && !state.held.is_empty()
                    {
                        if let Some(restart) = restart_held(ctx, state, &run) {
                            ending = Some(restart);
                        } else {
                            state.outcome.failure = Some(
                                "Codex finished without reporting a session, so its held instruction could not be applied."
                                    .into(),
                            );
                        }
                    }
                }
                break;
            }
        }
    }
    ending.unwrap_or(Next::Ended)
}

/// Answer one control. `Some` means this process is finishing, and the caller
/// drains what is left rather than acting on it immediately.
async fn answer(
    action: Control,
    store: &Store,
    ctx: &Context,
    state: &mut State,
    run: &mut proc::Run,
) -> Option<Next> {
    match action {
        Control::Cancel => {
            // Logged before the kill, because after it there may be no run
            // left to log anything.
            if let Err(e) = note(store, ctx, "cancel", CANCEL_PAYLOAD) {
                state.outcome.failure = Some(format!("could not record the stop: {}", e.message));
                return Some(Next::Ended);
            }
            state.outcome.cancelled = true;
            // Not a drop: the exit waiter still has the last of the CLI's
            // output to hand over.
            run.cancel();
            Some(Next::Ended)
        }
        Control::Instruct {
            text,
            apply_now,
            reply,
        } => {
            // Every later stage's brief repeats this, whether the running
            // provider took it live, held it, or finished before it landed. An
            // instruction never silently expires.
            state.constraints.push(text.clone());
            // A resume needs a session to resume, and a provider only reports
            // one once it has started talking. Without it the instruction waits
            // rather than appearing to have been applied.
            let disposition = match ctx.id.steering() {
                Steering::Live => {
                    // Straight down stdin, mid-turn. The only way this fails is
                    // a run whose stdin Orteca has already closed, which means
                    // the instruction arrived after the last turn ended.
                    match run.send_line(&claude::user_message(&text)).await {
                        Ok(()) => InstructionDisposition::Live,
                        Err(_) => InstructionDisposition::TooLate,
                    }
                }
                Steering::Checkpoint => {
                    state.held.push(text.clone());
                    if apply_now && state.session.is_none() {
                        state.apply_now_pending = true;
                    }
                    if apply_now && state.session.is_some() {
                        InstructionDisposition::Resumed
                    } else {
                        InstructionDisposition::Held
                    }
                }
            };
            // Never lost, whatever became of it: the task log is the record of
            // what the user asked for, the ones that had to wait included.
            if let Err(e) = note(store, ctx, "instruction", &instruction(&text, disposition)) {
                state.outcome.failure =
                    Some(format!("could not record the instruction: {}", e.message));
                return Some(Next::Ended);
            }
            let _ = reply.send(InstructionReceipt { disposition });
            if disposition == InstructionDisposition::Resumed {
                restart_held(ctx, state, run)
            } else {
                None
            }
        }
    }
}

/// Resume a checkpoint provider with every instruction accumulated since its
/// last launch. Called immediately for Apply now, when a delayed session ID
/// arrives, or at the natural process boundary for an ordinary Send.
///
/// It delivers something the *user* asked for while the run was going, so it
/// is never refused. The extra call is still counted, and still logged.
fn restart_held(ctx: &Context, state: &mut State, run: &proc::Run) -> Option<Next> {
    let session = state.session.as_deref()?;
    if state.held.is_empty() {
        return None;
    }
    let launch = Launch::resume(ctx.id, session, &state.held, &ctx.plan);
    state.held.clear();
    state.apply_now_pending = false;
    // The next attempt must prove its own completion. Keeping this true from
    // the previous turn would let a failed resume exit zero and look done.
    state.outcome.done = false;
    state.outcome.finished = false;
    run.cancel();
    Some(Next::Restart(launch))
}

/// A row in the task log that no provider said - Orteca or the user did.
fn note(store: &Store, ctx: &Context, kind: &str, payload: &str) -> crate::error::Result<()> {
    store.append_event(ctx.task_id, ctx.stage(), kind, ctx.id.program(), payload)?;
    Ok(())
}

fn instruction(text: &str, disposition: InstructionDisposition) -> String {
    serde_json::json!({ "kind": "instruction", "data": { "text": text, "applied": disposition } })
        .to_string()
}

/// Log the event, then show it. The log is the record; the emit is the view.
fn record(
    store: &Store,
    task_id: i64,
    stage: &str,
    id: ProviderId,
    event: &ProviderEvent,
) -> crate::error::Result<()> {
    let payload = serde_json::to_string(event).map_err(|e| {
        crate::error::AppError::new(crate::error::ErrorKind::Invalid, e.to_string())
    })?;
    // ponytail: SQLite writes on the async runtime. They are single-row inserts
    // on a local file; move them to spawn_blocking if a run ever feels slow.
    store.append_event(task_id, stage, event.kind(), id.program(), &payload)?;
    Ok(())
}

fn exit_message(id: ProviderId, code: Option<i32>, noise: &[String]) -> String {
    let tail = noise.join("\n");
    let how = match code {
        Some(code) => format!("{} exited with code {code}", id.program()),
        None => format!("{} was stopped before it finished", id.program()),
    };
    if tail.is_empty() {
        how
    } else {
        format!("{how}: {tail}")
    }
}

/// Trim the prompt, and treat a blank one as no prompt at all.
pub fn clean_prompt(prompt: &str) -> Option<String> {
    let trimmed = prompt.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{mock, CostQuality};
    use crate::routing::{Mode, RepoSignals, RouteKind};
    use crate::store::NewTask;

    /// The stage every pre-routing test used to be: one Implement call, which
    /// is also what a trivial task's route is.
    fn one_call(prompt: &str) -> Route {
        routing::route(prompt, Mode::Balanced, &RepoSignals::default())
    }

    fn plan_for(stage: Stage) -> StagePlan {
        StagePlan {
            stage,
            schema: write_schema(0, stage),
            tier: routing::Tier::Cheapest,
            model: None,
            effort: None,
            shell: true,
        }
    }

    /// A stage without a shell (an Answer) gets no Bash or PowerShell on
    /// Claude. The denylist stays as a backstop.
    #[test]
    fn a_stage_without_a_shell_has_no_shell_tools() {
        let claude = args(
            ProviderId::Claude,
            &StagePlan {
                shell: false,
                ..plan_for(Stage::Implement)
            },
        )
        .join(" ");
        assert!(
            claude.contains("--tools Read,Edit,Write,Glob,Grep "),
            "{claude}"
        );
        assert!(
            claude.contains("--allowedTools Read Edit Write Glob Grep --disallowedTools"),
            "{claude}"
        );
        assert!(claude.contains("Bash(git push:*)"));
    }

    #[test]
    fn only_small_regular_files_are_pasted() {
        let dir = std::env::temp_dir().join(format!("orteca-paste-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("slug.js"), "export const slug = s => s;").unwrap();
        std::fs::write(dir.join("big.js"), "x".repeat(9_000)).unwrap();
        let paths = ["big.js", "missing.js", "slug.js"].map(String::from);
        let text = pasted_files(&dir, &paths);
        assert!(
            text.contains("--- slug.js ---\nexport const slug") && !text.contains("big.js"),
            "{text}"
        );
        assert_eq!(pasted_files(&dir, &paths[..2]), "");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn attachments_grant_claude_their_folders_once_and_are_named_in_the_brief() {
        let dir = std::env::temp_dir();
        let files = vec![dir.join("a.png"), dir.join("b.txt"), dir.clone()];
        let argv = attachment_args(ProviderId::Claude, &files);
        assert_eq!(argv.len(), 2, "{argv:?}");
        assert_eq!(std::path::Path::new(&argv[1]), dir.as_path());
        assert!(attachment_args(ProviderId::Codex, &files).is_empty());
        assert!(attached_note(&files).contains(&files[0].display().to_string()));
        assert!(attached_note(&[]).is_empty());
    }

    #[test]
    fn deny_rules_are_individual_arguments_for_both_windows_shell_tools() {
        let argv = args(ProviderId::Claude, &plan_for(Stage::Implement));
        for tool in ["Bash", "PowerShell"] {
            for command in ["push", "reset", "clean"] {
                assert!(argv.contains(&format!("{tool}(git {command}:*)")));
            }
        }
    }

    #[test]
    fn fixture_reasoning_is_a_subset_of_output() {
        let usage = replay(ProviderId::Codex).usage.unwrap();
        assert!(usage.reasoning_tokens <= usage.output_tokens);
    }

    fn task_request(store: &Store, label: &str) -> Request {
        let dir =
            std::env::temp_dir().join(format!("orteca-stream-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success());
        // The shims' own logs are not the run's work: a Fix judged by whether it
        // changed the tree must not see them change.
        std::fs::create_dir_all(dir.join(".git/info")).unwrap();
        std::fs::write(dir.join(".git/info/exclude"), "briefs.log\nargv.log\n").unwrap();
        let project = store.touch_project(dir.to_str().unwrap(), "test").unwrap();
        Request {
            task_id: store
                .create_task(NewTask {
                    project_id: project.id,
                    prompt: "test",
                    mode: "balanced",
                    route_json: None,
                    branch: None,
                    base_commit: None,
                    dirty_at_start: false,
                })
                .unwrap(),
            attachments: Vec::new(),
            resume: None,
            continued: false,
            id: ProviderId::Codex,
            program: dir.join("fake.cmd"),
            dir,
            prompt: "a\"b %PATH% & ^\n\\ --help".into(),
            route: one_call("fix the typo"),
            base_commit: None,
            dirty_at_start: false,
            before_run: Some(Default::default()),
            recordings: None,
            worktree: None,
            classified: Vec::new(),
        }
    }

    async fn wait_for_kind(
        hearing: &mut mpsc::UnboundedReceiver<&'static str>,
        expected: &'static str,
    ) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match hearing.recv().await {
                    Some(kind) if kind == expected => break,
                    Some(_) => {}
                    None => panic!("event stream closed before `{expected}`"),
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for `{expected}`"));
    }

    #[tokio::test]
    async fn shim_receives_prompt_bytes_and_nonzero_exit_is_returned() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "stdin");
        let dir = request.dir.clone();
        let prompt = request.prompt.clone();
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        std::fs::write(dir.join("fake.js"), "let input='';process.stdin.setEncoding('utf8');process.stdin.on('data',s=>input+=s);process.stdin.on('end',()=>{console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:input}}));console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:100,cached_input_tokens:80,output_tokens:20}}));process.exitCode=1;});").unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let result = stream(&store, &Live::default(), request, |e| {
            events.borrow_mut().push(e.clone());
            Ok(())
        })
        .await;
        // The shim echoes whatever reached its stdin. What matters is that
        // the user's exact bytes survived the brief that wraps them, quotes,
        // percent signs, carets, backslashes and newlines included.
        assert!(
            result.summary.contains(&prompt),
            "prompt bytes were mangled: {}",
            result.summary
        );
        assert_eq!(result.status, "failed");
        assert!(result.failure.unwrap().contains("code 1"));
        assert!(!events.borrow().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The Codex run that did nothing: every tool call failed inside an item
    /// type this parser does not know, so the log recorded a clean `done` and
    /// kept no evidence at all. The raw line has to survive the parser.
    #[tokio::test]
    async fn an_event_the_parser_cannot_read_is_still_recorded() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "unknown-event");
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(
            &request.program,
            "@echo off
             echo {\"type\":\"item.completed\",\"item\":{\"type\":\"unified_exec\",\"error\":\"sandbox setup failed\"}}
             echo {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1}}
             exit /b 0
",
        )
        .unwrap();

        let emitted = std::cell::RefCell::new(Vec::new());
        stream(&store, &Live::default(), request, |e| {
            emitted.borrow_mut().push(e.kind());
            Ok(())
        })
        .await;

        assert!(
            store.event_kinds(task).contains(&"unknown".to_string()),
            "unparsed line was dropped"
        );
        let raw = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["item"]["type"] == "unified_exec")
            .expect("raw payload was not kept verbatim");
        assert_eq!(
            raw["item"]["error"], "sandbox setup failed",
            "the reason the run did nothing must survive"
        );
        // Logged, not shown: the UI has no shape for an event nobody parsed.
        assert!(!emitted.borrow().contains(&"unknown"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Real capture, 2026-09-13: Codex ran `workspace-write` as read-only, every
    /// command was rejected where `--json` never shows it, and the turn ended
    /// cleanly with an apology. Nothing in the stream is a failure; the
    /// untouched repository is.
    #[tokio::test]
    async fn a_codex_implement_stage_that_changed_nothing_is_not_done() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "read-only");
        let dir = request.dir.clone();
        // Outside the repository, or the shim itself would be the run's diff.
        let program =
            std::env::temp_dir().join(format!("orteca-read-only-{}.cmd", std::process::id()));
        let fixture = mock::named("codex-read-only-run.jsonl");
        std::fs::write(
            &program,
            format!("@echo off\r\ntype \"{}\"\r\n", fixture.display()),
        )
        .unwrap();
        request.program = program.clone();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert!(result.diff.is_empty());
        assert_eq!(result.status, "failed");
        assert!(result
            .failure
            .unwrap()
            .contains("without changing any file"));
        // The agent's own words stay the summary; they were never the signal.
        assert!(result.summary.contains("read-only"));
        std::fs::remove_file(program).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Real run, 2026-09-16: "Fetch latest changes" routed to implement, Claude
    /// answered with a question about `git fetch`, and the task read "Done".
    #[tokio::test]
    async fn a_claude_implement_stage_that_only_asked_a_question_is_not_done() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "claude-asked");
        request.id = ProviderId::Claude;
        let dir = request.dir.clone();
        // Outside the repository, or the shim itself would be the run's diff.
        let shim = std::env::temp_dir().join(format!("orteca-claude-asked-{}", std::process::id()));
        std::fs::create_dir_all(&shim).unwrap();
        std::fs::write(shim.join("fake.cmd"), "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        std::fs::write(
            shim.join("fake.js"),
            concat!(
                "process.stdin.resume();",
                "console.log(JSON.stringify({type:'result',subtype:'success',",
                "result:'Did you mean git fetch? Want me to run it?',",
                "usage:{input_tokens:1,output_tokens:1},total_cost_usd:0.01}));",
                "process.stdin.on('end',()=>process.exit(0));",
            ),
        )
        .unwrap();
        request.program = shim.join("fake.cmd");
        request.route.stages = vec![Stage::Implement, Stage::Verify];

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert!(result.diff.is_empty());
        // Nothing changed, so nothing is tested: the answer comes back at once.
        let ran: Vec<Stage> = result.stages.iter().map(|n| n.stage).collect();
        assert_eq!(ran, [Stage::Implement]);
        assert_eq!(result.status, "failed");
        assert!(result.failure.unwrap().contains("without changing any file"));
        assert!(result.summary.contains("git fetch"));
        std::fs::remove_dir_all(shim).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A live run spends the user's subscription once. What the CLI said has to
    /// come back for free afterwards, or every later test of this path bills
    /// them again - and the hand-written fixtures have already been wrong once.
    #[tokio::test]
    async fn a_paid_run_is_recorded_so_it_can_be_replayed_for_free() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "recording");
        let dir = request.dir.clone();
        let recordings = dir.join("recordings");
        request.recordings = Some(recordings.clone());
        let recording = recordings.join(format!("task-{}-implement-codex.jsonl", request.task_id));
        std::fs::write(
            &request.program,
            "@echo off
             echo {\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"an answer\"}}
             echo warming up
             echo {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":100,\"cached_input_tokens\":80,\"output_tokens\":20}}
             exit /b 0
",
        )
        .unwrap();

        let live = std::cell::RefCell::new(Vec::new());
        let result = stream(&store, &Live::default(), request, |e| {
            live.borrow_mut().push(serde_json::to_string(e).unwrap());
            Ok(())
        })
        .await;
        assert_eq!(result.status, "done");

        // Anything but JSONL makes the file a broken fixture, not a recording.
        let written = std::fs::read_to_string(&recording).expect("nothing was recorded");
        assert_eq!(
            written.lines().count(),
            2,
            "non-JSON output leaked in: {written}"
        );

        let mut replayed = crate::providers::mock::replay(ProviderId::Codex, &recording)
            .expect("a recording must load as a fixture");
        // The recording keeps what the CLI said. The model Codex leaves out is
        // Orteca's addition, made to a replay the same way as to a live run.
        let model = result.usage.as_ref().and_then(|u| u.model.clone()).expect("the run names its model");
        price_codex(&store, &model, &mut Outcome::default(), &mut replayed);
        let replayed: Vec<String> = replayed
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        assert_eq!(
            replayed,
            live.into_inner(),
            "replay must reproduce the run it recorded"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Stopping is the half of Milestone 5 that exists: the tree dies, what the
    /// agent already said survives, and the task does not read afterwards as a
    /// crash. Without the `cancel` row a run stopped two seconds in is
    /// indistinguishable later from one that died on its own.
    #[tokio::test]
    async fn a_stopped_run_is_cancelled_and_keeps_what_it_already_said() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "cancel");
        let dir = request.dir.clone();
        let task = request.task_id;
        // Answers once, then refuses to end on its own.
        std::fs::write(
            &request.program,
            "@echo off
echo {\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"half an answer\"}}
ping -n 60 127.0.0.1 >nul
",
        )
        .unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| {
                let _ = heard.send(e.kind());
                Ok(())
            }),
            async {
                // Stop only once the CLI has actually started talking, so this
                // tests a running agent and not a race with its start-up.
                assert_eq!(hearing.recv().await, Some("text"));
                live.send(task, Control::Cancel).unwrap();
            }
        );

        assert_eq!(result.status, "cancelled");
        assert_eq!(
            result.failure, None,
            "a stop the user asked for is not a failure"
        );
        assert_eq!(
            result.summary, "half an answer",
            "what the agent already said is kept"
        );
        assert!(
            store.event_kinds(task).contains(&"cancel".to_string()),
            "the log must record the stop"
        );
        // The sender goes with the run, so a late second click cannot claim to
        // have stopped something that is already over.
        assert!(live.send(task, Control::Cancel).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_control_cannot_reach_a_run_that_is_not_live() {
        let live = Live::default();
        assert!(
            live.send(7, Control::Cancel).is_err(),
            "nothing to stop yet"
        );
        let listening = live.open(7);
        assert!(live.send(7, Control::Cancel).is_ok());
        live.close(7);
        assert!(
            live.send(7, Control::Cancel).is_err(),
            "a finished run cannot be stopped"
        );
        drop(listening);
    }

    /// A result is not the same thing as an ending. A live provider reports one
    /// per turn, so a stop during turn two must still read as a stop even
    /// though turn one already answered - only `finished` means the run is over.
    /// The bug this guards: a three-call run on Claude reported the last call's
    /// cost as the whole run's, while its tokens were summed correctly.
    #[test]
    fn cost_adds_across_processes_and_replaces_within_one() {
        let usage = |cost_usd: Option<f64>| {
            ProviderEvent::Usage(Usage {
                model: None,
                input_tokens: 1,
                cached_input_tokens: 0,
                output_tokens: 1,
                reasoning_tokens: 0,
                cost_usd,
                cost_quality: if cost_usd.is_some() {
                    crate::providers::CostQuality::Estimated
                } else {
                    crate::providers::CostQuality::Unavailable
                },
            })
        };
        let mut outcome = Outcome::default();
        outcome.begin_process();
        outcome.absorb(&usage(Some(0.01)));
        outcome.absorb(&usage(Some(0.03))); // the same session, now at 0.03 in all
        outcome.begin_process();
        outcome.absorb(&usage(Some(0.02)));
        outcome.begin_process();
        outcome.absorb(&usage(None)); // a process that reported no cost adds nothing
        outcome.settle_cost();

        let total = outcome.usage.expect("usage was lost");
        assert!(
            (total.cost_usd.expect("cost was lost") - 0.05).abs() < 1e-9,
            "{:?}",
            total.cost_usd
        );
        assert_eq!(total.input_tokens, 4, "tokens still add per report");
        assert_eq!(total.cost_quality, crate::providers::CostQuality::Estimated);

        let mut free = Outcome::default();
        free.begin_process();
        free.absorb(&usage(None));
        free.settle_cost();
        assert_eq!(
            free.usage.unwrap().cost_usd,
            None,
            "no reported cost stays unavailable, never 0"
        );
    }

    /// A resumed Codex thread reports its running totals; only what the
    /// resumed process added is its own, and another thread starts from zero.
    #[test]
    fn a_resumed_codex_thread_counts_only_what_it_added() {
        let usage = |input, cached, output| {
            ProviderEvent::Usage(Usage {
                model: None,
                input_tokens: input,
                cached_input_tokens: cached,
                output_tokens: output,
                reasoning_tokens: 0,
                cost_usd: None,
                cost_quality: CostQuality::Unavailable,
            })
        };
        let tokens = |event: &ProviderEvent| match event {
            ProviderEvent::Usage(u) => (u.input_tokens, u.cached_input_tokens, u.output_tokens),
            _ => unreachable!(),
        };
        let mut totals = HashMap::new();
        let mut implement = [usage(55, 363, 10)];
        added_since(&mut totals, Some("t"), &mut implement);
        assert_eq!(tokens(&implement[0]), (55, 363, 10));
        let mut fix = [usage(142, 1004, 12)];
        added_since(&mut totals, Some("t"), &mut fix);
        assert_eq!(tokens(&fix[0]), (87, 641, 2));
        let mut review = [usage(5, 0, 1)];
        added_since(&mut totals, Some("r"), &mut review);
        assert_eq!(tokens(&review[0]), (5, 0, 1));
    }

    #[test]
    fn codex_turns_are_priced_per_process_and_an_unpriced_turn_voids_the_total() {
        let store = Store::in_memory().unwrap();
        let price = crate::store::Price { input: 1.0, output: 10.0, cache_read: 0.1 };
        store.save_prices(&[("luna".into(), price)]).unwrap();
        // $1 of input and $1 of output: $2 a turn.
        let turn = || {
            vec![ProviderEvent::Usage(Usage {
                model: None,
                input_tokens: 1_000_000,
                cached_input_tokens: 0,
                output_tokens: 100_000,
                reasoning_tokens: 0,
                cost_usd: None,
                cost_quality: CostQuality::Unavailable,
            })]
        };
        let run = |models: &[&str]| {
            let mut outcome = Outcome::default();
            for process in [models, models] {
                outcome.begin_process();
                for model in process {
                    let mut events = turn();
                    price_codex(&store, model, &mut outcome, &mut events);
                    outcome.absorb(&events[0]);
                }
            }
            outcome.settle_cost();
            outcome.usage.unwrap()
        };

        let priced = run(&["luna", "luna"]);
        assert!((priced.cost_usd.unwrap() - 8.0).abs() < 1e-9, "{:?}", priced.cost_usd);
        assert_eq!(priced.cost_quality, CostQuality::Estimated);

        let retired = run(&["luna", "retired"]);
        assert_eq!(retired.cost_usd, None);
        assert_eq!(retired.cost_quality, CostQuality::Unavailable);
    }

    /// A spent plan must reach the screen as `usageLimit`, which is what offers
    /// the other CLI, even when the result after it says nothing specific.
    #[test]
    fn a_reported_usage_limit_is_not_overwritten_by_a_vaguer_failure() {
        let mut outcome = Outcome::default();
        outcome.absorb(&ProviderEvent::Failed {
            kind: FailureKind::UsageLimit,
            message: "Claude's session usage limit is used up.".into(),
        });
        outcome.absorb(&ProviderEvent::Failed {
            kind: FailureKind::Crashed,
            message: "something went wrong".into(),
        });
        assert_eq!(outcome.failure_kind(), Some(FailureKind::UsageLimit));
        assert_eq!(
            outcome.failure.as_deref(),
            Some("something went wrong"),
            "the last words are still shown"
        );

        let mut unreported = Outcome {
            failure: Some("codex exited with code 1".into()),
            ..Outcome::default()
        };
        assert_eq!(
            unreported.failure_kind(),
            Some(FailureKind::Crashed),
            "read from the message when no provider said"
        );
        unreported.failure = None;
        assert_eq!(unreported.failure_kind(), None, "no failure, no kind");
    }

    #[test]
    fn a_stop_is_rewritten_by_the_run_ending_but_not_by_a_single_turn() {
        let mut answered = Outcome::default();
        answered.absorb(&ProviderEvent::Done {
            result: "shipped".into(),
            structured: None,
            turns: 1,
        });
        answered.finished = true;
        answered.cancelled = true;
        assert_eq!(answered.status(), "done", "the run had already ended");

        let mut mid_turn = Outcome::default();
        mid_turn.absorb(&ProviderEvent::Done {
            result: "turn one".into(),
            structured: None,
            turns: 1,
        });
        mid_turn.cancelled = true;
        assert!(mid_turn.done, "a turn did answer");
        assert_eq!(
            mid_turn.status(),
            "cancelled",
            "but the run was stopped mid-work"
        );

        let mut stopped = Outcome {
            cancelled: true,
            ..Outcome::default()
        };
        // Terminating the job leaves no exit code at all, and that is expected.
        stopped.exited(ProviderId::Codex, None, &["killed".into()]);
        assert_eq!(stopped.failure, None);
        assert_eq!(stopped.status(), "cancelled");
    }

    /// Current Claude Code incorporates an instruction received during work
    /// into the current turn and emits one result for both messages. Waiting
    /// for one result per input leaves the task running forever.
    #[tokio::test]
    async fn a_live_instruction_incorporated_into_one_result_finishes() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "steer-live");
        request.id = ProviderId::Claude;
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        // Emits activity after the opening message, accepts the instruction,
        // then reports both through a single final result like the real run.
        std::fs::write(dir.join("fake.js"), concat!(
            "let buf='',started=false,messages=[];process.stdin.setEncoding('utf8');",
            "process.stdin.on('data',d=>{buf+=d;const ls=buf.split('\\n');buf=ls.pop();",
            "for(const l of ls){if(!l.trim())continue;const text=JSON.parse(l).message.content;",
            "messages.push(text);if(!started){started=true;",
            "console.log(JSON.stringify({type:'assistant',message:{content:[{type:'text',text:'working'}]}}));",
            "setTimeout(()=>console.log(JSON.stringify({type:'result',subtype:'success',result:messages.join(' + '),",
            "usage:{input_tokens:1,output_tokens:1},total_cost_usd:0.01})),300);}}});",
            "process.stdin.on('end',()=>setTimeout(()=>process.exit(0),400));",
        )).unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| {
                let _ = heard.send(e.kind());
                Ok(())
            }),
            async {
                // The first text means the shared turn is under way.
                wait_for_kind(&mut hearing, "text").await;
                let receipt = live
                    .instruct(task, "also tidy up".into(), false)
                    .await
                    .unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Live);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert!(
            result.summary.ends_with("also tidy up"),
            "the live instruction was not incorporated"
        );
        let usage = result.usage.expect("a steered run still reports usage");
        assert_eq!(usage.output_tokens, 1, "one result must be counted once");
        assert_eq!(usage.cost_usd, Some(0.01));

        let payload = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["kind"] == "instruction")
            .expect("the instruction was not written to the log");
        assert_eq!(payload["data"]["applied"], "live");
        assert_eq!(payload["data"]["text"], "also tidy up");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The control channel can still be live for the few milliseconds between
    /// Claude's final result closing stdin and the process exit. Enqueueing in
    /// that window must report too-late rather than claim delivery.
    #[tokio::test]
    async fn a_live_instruction_after_the_final_result_is_reported_too_late() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "steer-too-late");
        request.id = ProviderId::Claude;
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        std::fs::write(
            dir.join("fake.js"),
            concat!(
                "process.stdin.resume();",
                "console.log(JSON.stringify({type:'result',subtype:'success',result:'done',",
                "usage:{input_tokens:1,output_tokens:1},total_cost_usd:0.01}));",
                "process.stdin.on('end',()=>setTimeout(()=>process.exit(0),500));",
            ),
        )
        .unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, receipt) = tokio::join!(
            stream(&store, &live, request, move |e| {
                let _ = heard.send(e.kind());
                Ok(())
            }),
            async {
                wait_for_kind(&mut hearing, "done").await;
                live.instruct(task, "one more thing".into(), false)
                    .await
                    .unwrap()
            }
        );

        assert_eq!(receipt.disposition, InstructionDisposition::TooLate);
        assert_eq!(result.status, "done", "{:?}", result.failure);
        let payload = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["kind"] == "instruction")
            .expect("the late instruction was not logged");
        assert_eq!(payload["data"]["applied"], "tooLate");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A checkpoint provider has no stdin to speak down, so an instruction
    /// waits until the current process ends and is then applied by resuming the
    /// session. Merely logging and dropping it is not enough.
    #[tokio::test]
    async fn a_checkpoint_instruction_is_applied_at_the_process_boundary() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "steer-held");
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(
            &request.program,
            "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n",
        )
        .unwrap();
        // The first process finishes naturally. The held instruction must then
        // enter the same session through a resumed process.
        std::fs::write(dir.join("fake.js"), concat!(
            "const a=process.argv.slice(2);",
            "if(!a.includes('resume')){",
            "console.log(JSON.stringify({type:'thread.started',thread_id:'sess-held'}));",
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'working'}}));",
            "setTimeout(()=>console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:1,output_tokens:1}})),700);",
            "}else{let i='';process.stdin.setEncoding('utf8');process.stdin.on('data',d=>i+=d);",
            "process.stdin.on('end',()=>{",
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'boundary '+i.trim()}}));",
            "console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:1,output_tokens:1}}));});}",
        )).unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| {
                let _ = heard.send(e.kind());
                Ok(())
            }),
            async {
                wait_for_kind(&mut hearing, "text").await;
                let receipt = live.instruct(task, "use tabs".into(), false).await.unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Held);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert_eq!(result.summary, "boundary use tabs");
        let payload = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["kind"] == "instruction")
            .expect("a held instruction still has to be recorded");
        assert_eq!(payload["data"]["applied"], "held");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// "Apply now" on a checkpoint provider: the process ends and its own
    /// session is picked back up carrying the instruction. The resumed process
    /// has to be the same task - one event log, one token total - and the kill
    /// that hands over must not be reported as a crash.
    #[tokio::test]
    async fn applying_now_resumes_the_session_rather_than_starting_a_new_task() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "steer-resume");
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(
            &request.program,
            "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n",
        )
        .unwrap();
        // First run reports a session and then refuses to end. Resumed, it
        // reports what it was resumed with.
        std::fs::write(dir.join("fake.js"), concat!(
            "const a=process.argv.slice(2);",
            "if(!a.includes('resume')){",
            "console.log(JSON.stringify({type:'thread.started',thread_id:'sess-1'}));",
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'working'}}));",
            "setInterval(()=>{},1000);",
            "}else{let i='';process.stdin.setEncoding('utf8');process.stdin.on('data',d=>i+=d);",
            "process.stdin.on('end',()=>{",
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'resumed '+a[2]+' with '+i.trim()}}));",
            "console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:1,output_tokens:1}}));});}",
        )).unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| {
                let _ = heard.send(e.kind());
                Ok(())
            }),
            async {
                wait_for_kind(&mut hearing, "text").await;
                let receipt = live
                    .instruct(task, "make it faster".into(), true)
                    .await
                    .unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Resumed);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert_eq!(result.summary, "resumed sess-1 with make it faster");
        let payload = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["kind"] == "instruction")
            .expect("the instruction was not logged");
        assert_eq!(payload["data"]["applied"], "resumed");
        // One task across two processes: the first one's words are still here.
        let texts: Vec<String> = store
            .event_payloads(task)
            .into_iter()
            .filter(|v| v["kind"] == "text")
            .map(|v| v["data"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            texts.contains(&"working".to_string()),
            "the log lost the first process"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Apply-now before the provider has said who it is waits for the Started
    /// event and then immediately resumes instead of evaporating.
    #[tokio::test]
    async fn applying_now_before_the_session_id_resumes_when_it_arrives() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "steer-nosession");
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(
            &request.program,
            "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n",
        )
        .unwrap();
        // There is enough time to submit Apply now after text but before the
        // delayed session ID. Resumed, the shim reports the carried words.
        std::fs::write(dir.join("fake.js"), concat!(
            "const a=process.argv.slice(2);",
            "if(!a.includes('resume')){",
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'anonymous'}}));",
            "setTimeout(()=>console.log(JSON.stringify({type:'thread.started',thread_id:'sess-late'})),500);",
            "setInterval(()=>{},1000);",
            "}else{let i='';process.stdin.setEncoding('utf8');process.stdin.on('data',d=>i+=d);",
            "process.stdin.on('end',()=>{",
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'late '+i.trim()}}));",
            "console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:1,output_tokens:1}}));});}",
        )).unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| {
                let _ = heard.send(e.kind());
                Ok(())
            }),
            async {
                wait_for_kind(&mut hearing, "text").await;
                let receipt = live.instruct(task, "hurry".into(), true).await.unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Held);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert_eq!(result.summary, "late hurry");
        let payload = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["kind"] == "instruction")
            .expect("not logged");
        assert_eq!(
            payload["data"]["applied"], "held",
            "the receipt was honest while the session was pending"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn spawn_failure_is_returned_and_logged() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "spawn-failure");
        request.program = request.dir.join("missing.exe");
        let dir = request.dir.clone();
        let task = request.task_id;
        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;
        assert_eq!(result.status, "failed");
        assert!(result.failure.unwrap().contains("could not start codex"));
        assert!(store
            .event_payloads(task)
            .iter()
            .any(|v| v["kind"] == "failed"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn event_storage_and_delivery_failures_cannot_report_success() {
        for reject_storage in [false, true] {
            let store = Store::in_memory().unwrap();
            let request = task_request(
                &store,
                if reject_storage {
                    "storage-failure"
                } else {
                    "delivery-failure"
                },
            );
            let dir = request.dir.clone();
            std::fs::write(&request.program, "@echo off\r\necho {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1}}\r\nexit /b 0\r\n").unwrap();
            if reject_storage {
                store.reject_events();
            }
            let result = stream(&store, &Live::default(), request, |_| {
                if reject_storage {
                    Ok(())
                } else {
                    Err(crate::error::AppError::new(
                        crate::error::ErrorKind::Io,
                        "window unavailable",
                    ))
                }
            })
            .await;
            assert_eq!(result.status, "failed");
            assert!(result.failure.unwrap().contains(if reject_storage {
                "disk unavailable"
            } else {
                "window unavailable"
            }));
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn a_result_does_not_hide_a_failed_exit() {
        let mut outcome = replay(ProviderId::Claude);
        outcome.exited(ProviderId::Claude, Some(1), &["cleanup failed".into()]);
        assert!(outcome.failure.unwrap().contains("cleanup failed"));
    }

    #[test]
    fn arbitrary_prompts_never_reach_a_cmd_argument() {
        for id in ProviderId::ALL {
            for prompt in ["a\"b", "%PATH%", "a&b", "a^b", "a\nb", "\\", "--help"] {
                assert!(
                    !args(id, &plan_for(Stage::Implement)).contains(&prompt.to_string()),
                    "{id:?}: {prompt:?}"
                );
            }
        }
    }

    fn replay(id: ProviderId) -> Outcome {
        let mut outcome = Outcome::default();
        for event in mock::replay(id, &mock::fixture(id)).expect("fixture") {
            outcome.absorb(&event);
        }
        outcome
    }

    #[test]
    fn claude_is_never_run_bare_and_codex_is_never_fully_trusted() {
        let claude = args(ProviderId::Claude, &plan_for(Stage::Implement)).join(" ");
        assert!(!claude.contains("--bare"), "--bare would force an API key");
        assert!(claude.contains("--permission-mode acceptEdits"));
        // The user's plugins, skills, MCP servers and CLAUDE.md stay out of the
        // run; the repository's own settings still load (§4.3.3).
        assert!(claude.contains("--setting-sources project,local"));
        assert!(claude.contains("--strict-mcp-config"));
        assert!(claude.contains("--disable-slash-commands"));
        assert!(claude.contains("--tools Bash,PowerShell,Read,Edit,Write,Glob,Grep"));
        // A headless run has nobody to answer a prompt, so it must never wait
        // for one. This is what made an agent stall instead of running a test.
        assert!(claude.contains("--permission-prompts none"));
        // The agent has to be able to verify its own work.
        assert!(claude.contains("--allowedTools Bash PowerShell"));

        // Deny beats allow, and every rule covers both shells.
        for command in CLAUDE_DENY_COMMANDS {
            assert!(claude.contains(&format!("Bash({command})")), "{command}");
            assert!(
                claude.contains(&format!("PowerShell({command})")),
                "{command}"
            );
        }
        assert!(claude.contains("Bash(git push:*)"));
        assert!(claude.contains("Bash(rm:*)"));
        assert!(claude.contains("PowerShell(Remove-Item:*)"));
        // Granting the shells must not silently outrank the denylist.
        let allow = claude.find("--allowedTools").expect("allow");
        let deny = claude.find("--disallowedTools").expect("deny");
        assert!(
            allow < deny,
            "denylist must come after the grant it narrows"
        );

        let codex = args(ProviderId::Codex, &plan_for(Stage::Implement)).join(" ");
        assert!(codex.contains("--sandbox workspace-write"));
        assert!(!codex.contains("danger-full-access"));
        assert!(codex.contains(&crate::providers::CODEX_ISOLATION.join(" ")));
        // Without it, ignoring the user's config leaves Codex read-only on Windows.
        assert!(codex.contains("windows.sandbox=\"elevated\""));
        // The prompt is one argument, never spliced into a shell string.
        assert!(args(ProviderId::Codex, &plan_for(Stage::Implement)).contains(&"-".to_string()));
    }

    /// A stage that has no business editing is stopped from editing, rather
    /// than merely asked not to. Codex has an OS-level fence for it; Claude has
    /// no read-only mode, so the edit tools are denied by name.
    #[test]
    fn a_stage_that_must_not_write_is_not_merely_asked_not_to() {
        for stage in [Stage::Plan, Stage::Review, Stage::Verify] {
            let codex = args(ProviderId::Codex, &plan_for(stage)).join(" ");
            assert!(codex.contains("--sandbox read-only"), "{}", stage.name());
            assert!(!codex.contains("workspace-write"), "{}", stage.name());

            // Whole arguments, not substrings: `Edit` lives inside
            // `acceptEdits`, and matching text would pass either way.
            let claude = args(ProviderId::Claude, &plan_for(stage));
            for tool in CLAUDE_EDIT_TOOLS {
                assert!(
                    claude.iter().any(|a| a == tool),
                    "{} could still call {tool}",
                    stage.name()
                );
            }
            assert!(
                claude.contains(&"Read".to_string()),
                "{} lost read access",
                stage.name()
            );
            assert!(
                !claude.contains(&"Bash".to_string()),
                "{} received arbitrary Bash",
                stage.name()
            );
            assert!(
                !claude.contains(&"PowerShell".to_string()),
                "{} received arbitrary PowerShell",
                stage.name()
            );
        }
        // Implement still writes, or nothing would ever change.
        let codex = args(ProviderId::Codex, &plan_for(Stage::Implement)).join(" ");
        assert!(codex.contains("--sandbox workspace-write"));
        let claude = args(ProviderId::Claude, &plan_for(Stage::Implement));
        for tool in CLAUDE_EDIT_TOOLS {
            assert!(!claude.iter().any(|a| a == tool), "Implement lost {tool}");
        }
    }

    /// The schema flags are the ones the installed CLIs actually accept,
    /// checked against them rather than taken from the spec. Claude takes its
    /// schema inline and Codex takes a file. Neither gets a turn ceiling.
    #[test]
    fn only_verified_flags_reach_a_command_line() {
        let plan = StagePlan {
            stage: Stage::Plan,
            schema: write_schema(0, Stage::Plan),
            tier: routing::Tier::Deep,
            model: None,
            effort: None,
            shell: true,
        };
        let claude = args(ProviderId::Claude, &plan);
        assert!(
            !claude.contains(&"--max-turns".to_string()),
            "a stage runs until it is done"
        );
        assert!(claude.contains(&"--json-schema".to_string()));
        assert!(claude.contains(&routing::PLAN_SCHEMA.to_string()));

        let codex = args(ProviderId::Codex, &plan);
        assert!(
            !codex.contains(&"--max-turns".to_string()),
            "codex 0.154.0 has no turn flag"
        );
        assert!(
            !codex.contains(&"--json-schema".to_string()),
            "codex takes a file, not inline JSON"
        );
        let at = codex
            .iter()
            .position(|a| a == "--output-schema")
            .expect("codex takes a schema file");
        let path = std::path::Path::new(&codex[at + 1]);
        assert_eq!(std::fs::read_to_string(path).unwrap(), routing::PLAN_SCHEMA);
        // Never in the user's repository: a schema file in their diff would be
        // Orteca editing their project.
        assert!(!path.starts_with(std::env::current_dir().unwrap()));

        // A stage with no artifact contract asks for none.
        let implement = args(ProviderId::Claude, &plan_for(Stage::Implement));
        assert!(!implement.contains(&"--json-schema".to_string()));

        // The tier reaches both command lines as a model and an effort, and a
        // resumed Codex session keeps them instead of the account default.
        assert!(claude
            .windows(4)
            .any(|w| w == ["--model", "opus", "--effort", "high"]));
        assert!(codex.windows(4).any(|w| w
            == [
                "--model",
                "gpt-5.6-sol",
                "-c",
                "model_reasoning_effort=\"high\""
            ]));
        let resumed = Launch::resume(ProviderId::Codex, "abc-123", &[], &plan).argv;
        assert!(resumed.windows(2).any(|w| w == ["--model", "gpt-5.6-sol"]));

        let efficient_review = StagePlan {
            model: Some("gpt-5.6-terra"),
            effort: Some("high"),
            ..plan_for(Stage::Review)
        };
        assert!(args(ProviderId::Codex, &efficient_review)
            .windows(4)
            .any(|w| w
                == [
                    "--model",
                    "gpt-5.6-terra",
                    "-c",
                    "model_reasoning_effort=\"high\""
                ]));
    }

    /// A shim that answers every call, logs the brief it was given, and exits.
    /// `turns` is how many turns it reports before it stops, which is what a
    /// turn ceiling is measured against.
    fn answering_shim(request: &Request, turns: usize) {
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        let log = request
            .dir
            .join("briefs.log")
            .to_string_lossy()
            .replace('\\', "/");
        std::fs::write(
            request.dir.join("fake.js"),
            format!(
                "const fs=require('fs');let input='';process.stdin.setEncoding('utf8');\
                 process.stdin.on('data',s=>input+=s);process.stdin.on('end',()=>{{\
                 fs.appendFileSync('{log}','=== CA'+'LL ===\\n'+input);\
                 console.log(JSON.stringify({{type:'item.completed',item:{{type:'agent_message',text:input}}}}));\
                 for(let i=0;i<{turns};i++)console.log(JSON.stringify({{type:'turn.completed',usage:{{input_tokens:10,output_tokens:1}}}}));\
                 process.exitCode=0;}});"
            ),
        )
        .unwrap();
    }

    /// A Codex-shaped shim whose completed agent message contains the value
    /// constrained by `--output-schema`.
    fn codex_artifact_shim(request: &Request, artifact: &serde_json::Value) {
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        let text = serde_json::to_string(&artifact.to_string()).unwrap();
        std::fs::write(
            request.dir.join("fake.js"),
            format!(
                "const text={text};\
                 console.log(JSON.stringify({{type:'item.completed',item:{{type:'agent_message',text}}}}));\
                 console.log(JSON.stringify({{type:'turn.completed',usage:{{input_tokens:1,output_tokens:1}}}}));"
            ),
        )
        .unwrap();
    }

    /// A Claude-shaped shim. It answers the first message rather than waiting
    /// for stdin to end, because a live provider's stdin is only closed once it
    /// has answered - waiting for the end would deadlock both sides.
    fn claude_shim(request: &mut Request, structured: &serde_json::Value) {
        claude_shim_answers(request, std::slice::from_ref(structured));
    }

    /// The same, answering the Nth call with the Nth value and every call after
    /// the last value with that one.
    fn claude_shim_answers(request: &mut Request, answers: &[serde_json::Value]) {
        request.id = ProviderId::Claude;
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        let log = request
            .dir
            .join("briefs.log")
            .to_string_lossy()
            .replace('\\', "/");
        let answers = serde_json::to_string(answers).unwrap();
        std::fs::write(
            request.dir.join("fake.js"),
            format!(
                "const fs=require('fs');const answers={answers};let buf='',answered=false;process.stdin.setEncoding('utf8');\
                 process.stdin.on('data',d=>{{buf+=d;const ls=buf.split('\\n');buf=ls.pop();\
                 for(const l of ls){{if(!l.trim()||answered)continue;answered=true;\
                 const n=fs.existsSync('{log}')?fs.readFileSync('{log}','utf8').split('=== CA'+'LL ===').length-1:0;\
                 fs.appendFileSync('{log}','=== CA'+'LL ===' + JSON.parse(l).message.content);\
                 console.log(JSON.stringify({{type:'result',subtype:'success',result:'stage answered',\
                 structured_output:answers[Math.min(n,answers.length-1)],usage:{{input_tokens:10,output_tokens:1}},total_cost_usd:0.01}}));}}}});\
                 process.stdin.on('end',()=>process.exit(0));"
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_failed_verify_is_fixed_once_unless_it_failed_before_the_run() {
        let failing = serde_json::json!({"checks": [{"command": "npm test", "passed": false, "output": "no"}], "verdict": "fail"});
        let verify = StageNote { stage: Stage::Verify, model: None, effort: None, summary: String::new(), artifact: Some(failing.clone()) };
        let fix = StageNote { stage: Stage::Fix, model: None, effort: None, summary: String::new(), artifact: None };
        let round = |notes, failing_before| Round {
            stage: Stage::Verify,
            failed: true,
            fixing_review: false,
            fix_changed: None,
            notes,
            failing_before,
            verifies_before_review: false,
            remaining: &[],
        };
        let first = [verify.clone()];
        let after_fix = [verify.clone(), fix, verify];
        let before = [failing["checks"][0].clone()];
        let other = [serde_json::json!({"command": "cargo test", "passed": false})];

        assert!(matches!(after_stage(round(&first, &[])), Then::Retry(s) if s == [Stage::Fix, Stage::Verify]));
        assert!(matches!(after_stage(round(&first, &other)), Then::Retry(_)), "another suite failing before is no excuse");
        assert!(matches!(after_stage(round(&first, &before)), Then::Stop(stop) if stop.message.contains("already failed")));
        assert!(matches!(after_stage(round(&after_fix, &[])), Then::Stop(stop) if stop.message.contains("One automatic fix")));
        assert!(matches!(after_stage(Round { failed: false, ..round(&first, &before) }), Then::Continue));
    }

    /// A `package.json` whose test fails the first time it runs, saying why,
    /// and passes every time after.
    const FAILS_ONCE: &str = r#"{"scripts":{"test":"node -e \"const f=require('fs');if(f.existsSync('ran'))process.exit(0);f.writeFileSync('ran','');console.log('header is not bold');process.exit(1)\""}}"#;

    /// A stand-in Laravel app: `composer test` runs the suite, the quick check
    /// runs one test file, and both log what ran. A `broken` file fails both,
    /// the way a database that is not running would. `None` when PHP or
    /// Composer is not on PATH, so the caller can skip.
    fn fake_laravel(dir: &std::path::Path) -> Option<()> {
        crate::providers::which("php")?;
        crate::providers::which("composer")?;
        std::fs::write(
            dir.join("composer.json"),
            r#"{"scripts":{"test":"@php artisan suite"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("artisan"),
            "<?php\nfile_put_contents('runs.log', implode(' ', array_slice($argv, 1)) . \"\\n\", FILE_APPEND);\n\
             if (file_exists('broken')) { echo \"SQLSTATE connection refused\\n\"; exit(1); }\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("tests/Feature")).unwrap();
        std::fs::write(dir.join("tests/Feature/SmallTest.php"), "<?php\n").unwrap();
        std::fs::write(dir.join("tests/Feature/LargerTest.php"), "<?php\n// more\n").unwrap();
        Some(())
    }

    #[test]
    fn the_check_before_a_change_is_one_small_laravel_test() {
        let dir = std::env::temp_dir().join(format!("orteca-quick-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("tests/Feature")).unwrap();
        std::fs::write(dir.join("tests/Feature/SmallTest.php"), "<?php\n").unwrap();
        std::fs::write(dir.join("tests/Feature/LargerTest.php"), "<?php\n// more\n").unwrap();
        std::fs::write(dir.join("tests/Feature/helpers.php"), "").unwrap();
        let check = |kind| project::Check {
            kind,
            dir: dir.clone(),
            install: None,
            test: vec!["composer", "test"],
        };
        assert_eq!(quick_check(&check("php")), None, "no artisan, not Laravel");
        std::fs::write(dir.join("artisan"), "").unwrap();
        assert_eq!(
            quick_check(&check("php")),
            Some(vec!["php".into(), "artisan".into(), "test".into(), "tests/Feature/SmallTest.php".into()])
        );
        assert_eq!(quick_check(&check("js")), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Checks that already failed before the change are shown to the agent,
    /// and failing again buys no Fix: Orteca cannot tell a broken setup from
    /// unfinished work.
    #[tokio::test]
    async fn checks_that_failed_before_the_run_buy_no_fix() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "failed-before", route);
        let dir = request.dir.clone();
        if fake_laravel(&dir).is_none() {
            eprintln!("skipped: php or composer is not on PATH");
            return;
        }
        std::fs::write(dir.join("broken"), "").unwrap();
        let passing = serde_json::json!({"checks": [{"command": "npm test", "passed": true, "output": "ok"}], "verdict": "pass"});
        claude_shim(&mut request, &passing);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "verifyFailed", "{:?}", result.failure);
        assert_eq!(result.calls_used, 1, "a failure the run did not cause bought a Fix");
        assert_eq!(
            result.stages.iter().map(|note| note.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Verify]
        );
        let stop = result.budget_stop.expect("the stop was not returned");
        assert!(stop.message.contains("already failed"), "{}", stop.message);
        assert!(
            briefs(&dir)[0].contains("SQLSTATE connection refused"),
            "the agent was not shown what already fails"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A pass before the change is remembered for the same tree, so the next
    /// run does not pay for the quick check twice.
    #[tokio::test]
    async fn a_passing_check_before_the_run_is_not_repeated_for_the_same_tree() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let passing = serde_json::json!({"checks": [{"command": "npm test", "passed": true, "output": "ok"}], "verdict": "pass"});
        let mut dir = None;
        // The quick check, then Verify: the untracked test files count as
        // changed, so their tripwire runs before the suite. The second run
        // skips the quick check.
        let verify = "test tests/Feature/LargerTest.php tests/Feature/SmallTest.php\nsuite\n";
        let first = format!("test tests/Feature/SmallTest.php\n{verify}");
        for runs in [first.clone(), format!("{first}{verify}")] {
            let mut request = routed(&store, "passed-before", route.clone());
            if fake_laravel(&request.dir).is_none() {
                eprintln!("skipped: php or composer is not on PATH");
                return;
            }
            claude_shim(&mut request, &passing);
            let here = request.dir.clone();
            let result = stream(&store, &Live::default(), request, |_| Ok(())).await;
            assert_eq!(result.status, "done", "{:?}", result.budget_stop);
            assert_eq!(std::fs::read_to_string(here.join("runs.log")).unwrap(), runs);
            dir = Some(here);
        }
        std::fs::remove_dir_all(dir.unwrap()).unwrap();
    }

    #[test]
    fn changed_laravel_tests_become_the_early_tripwire() {
        let dir = std::env::temp_dir().join(format!(
            "orteca-focused-php-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join("tests/Feature")).unwrap();
        std::fs::write(dir.join("artisan"), "").unwrap();
        let check = project::Check {
            kind: "php",
            dir: dir.clone(),
            install: None,
            test: vec!["composer", "test"],
        };
        assert_eq!(
            focused_php_check(
                &dir,
                &check,
                &[
                    "app/Console/Command.php".into(),
                    "tests/Feature/ReminderTest.php".into(),
                ],
            ),
            Some(vec![
                "php".into(),
                "artisan".into(),
                "test".into(),
                "tests/Feature/ReminderTest.php".into(),
            ])
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// What each call was actually asked to do.
    fn briefs(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("briefs.log"))
            .unwrap_or_default()
            .split("=== CALL ===")
            .skip(1)
            .map(str::to_string)
            .collect()
    }

    fn routed(store: &Store, label: &str, route: Route) -> Request {
        let mut request = task_request(store, label);
        request.route = route;
        request
    }

    /// A run in a separate copy commits its work to the copy's branch and
    /// writes nothing in the user's folder.
    #[tokio::test]
    async fn a_run_in_a_copy_commits_there_and_not_in_the_users_folder() {
        let store = Store::in_memory().unwrap();
        let mut request = routed(&store, "worktree", one_call("fix the typo in the readme"));
        let repo = request.dir.clone();
        assert!(std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "initial"
            ])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        let copy = project::worktree_dir(&project::git_state(&repo).root.unwrap(), request.task_id)
            .unwrap();
        let _ = std::fs::remove_dir_all(&copy);
        project::add_worktree(&repo, &copy, &format!("orteca/test-{}", std::process::id()))
            .unwrap();
        request.program = copy.join("fake.cmd");
        request.dir = copy.clone();
        request.worktree = Some(project::Worktree {
            path: copy.to_string_lossy().into_owned(),
            branch: "test".into(),
            commit: None,
            commit_error: None,
        });
        answering_shim(&request, 1);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "done", "{:?}", result.failure);
        let tree = result.worktree.expect("the copy was not reported");
        assert!(
            tree.commit.is_some(),
            "not committed: {:?}",
            tree.commit_error
        );
        assert!(copy.join("briefs.log").exists());
        assert!(
            !repo.join("briefs.log").exists(),
            "the run wrote in the user's folder"
        );
        let _ = std::fs::remove_dir_all(&copy);
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// The headline of Milestone 6, end to end: a trivial task starts exactly
    /// one provider process, and that process is told to verify its own work.
    #[tokio::test]
    async fn a_trivial_task_spends_exactly_one_agent_call() {
        let store = Store::in_memory().unwrap();
        let request = routed(&store, "trivial", one_call("fix the typo in the readme"));
        assert_eq!(request.route.kind, RouteKind::ImplementOnce);
        let (dir, task) = (request.dir.clone(), request.task_id);
        answering_shim(&request, 1);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "done");
        assert_eq!(
            result.calls_used, 1,
            "a trivial task started more than one process"
        );
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Implement]
        );
        assert!(result.budget_stop.is_none());

        let calls = briefs(&dir);
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].contains("one focused check"),
            "verification was not asked for in the one call"
        );

        // The log has to show which stages ran, and no others.
        let stages: Vec<String> = store
            .event_payloads(task)
            .into_iter()
            .filter(|v| v["kind"] == "stage")
            .map(|v| v["data"]["stage"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(stages, ["implement"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Every routing and budget decision is in the log, because Milestone 7
    /// cannot score a route it cannot see.
    #[tokio::test]
    async fn the_route_is_recorded_before_anything_runs() {
        let store = Store::in_memory().unwrap();
        let request = routed(&store, "recorded", one_call("fix the typo in the readme"));
        let (dir, task) = (request.dir.clone(), request.task_id);
        answering_shim(&request, 1);
        stream(&store, &Live::default(), request, |_| Ok(())).await;

        let payloads = store.event_payloads(task);
        let routing = payloads
            .iter()
            .find(|v| v["kind"] == "routing")
            .expect("no routing decision was recorded");
        assert_eq!(routing["data"]["kind"], "implementOnce");
        assert!(routing["data"]["signals"]["complexity"].is_number());
        assert!(routing["data"]["reason"]
            .as_str()
            .is_some_and(|r| !r.is_empty()));
        // The tier is recorded with why, and each stage names the model it asked for.
        assert!(routing["data"]["budget"]["preferredTier"].is_string());
        assert!(routing["data"]["tierReason"]
            .as_str()
            .is_some_and(|r| !r.is_empty()));
        let stage = payloads
            .iter()
            .find(|v| v["kind"] == "stage")
            .expect("no stage was recorded");
        assert!(stage["data"]["model"]
            .as_str()
            .is_some_and(|m| !m.is_empty()));
        // Before the first provider event, not after.
        let first_provider = payloads
            .iter()
            .position(|v| v["kind"] == "started" || v["kind"] == "text");
        let at = payloads
            .iter()
            .position(|v| v["kind"] == "routing")
            .unwrap();
        assert!(
            first_provider.is_none_or(|p| at < p),
            "the route was recorded after the run began"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A longer route runs its stages in order, and each one is told what it is
    /// for. Only Implement is allowed to write.
    #[tokio::test]
    async fn a_longer_route_runs_its_stages_in_order() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "redesign the authorization subsystem so every endpoint checks it",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        assert_eq!(
            route.stages,
            [Stage::Plan, Stage::Implement, Stage::Review, Stage::Verify]
        );
        let mut request = routed(&store, "stages", route);
        let (dir, task) = (request.dir.clone(), request.task_id);
        // One answer that is both a passing Review and a passing Verify, since
        // the shim gives every stage the same one.
        claude_shim(
            &mut request,
            &serde_json::json!({
                "findings": [],
                "checks": [{"command": "cargo test auth", "passed": true, "output": "ok"}],
                "verdict": "pass"
            }),
        );

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "done");
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Plan, Stage::Implement, Stage::Review, Stage::Verify]
        );
        assert_eq!(result.calls_used, 4);
        let calls = briefs(&dir);
        assert!(
            calls[0].contains("Do not edit any file"),
            "the plan stage was allowed to edit"
        );
        assert!(calls[1].contains("Make the change"));
        assert!(calls[3].contains("Verify"));
        // Built a tier below, reviewed on deep.
        let payloads = store.event_payloads(task);
        let model = |stage: &str| {
            payloads
                .iter()
                .find(|v| v["kind"] == "stage" && v["data"]["stage"] == stage)
                .map(|v| v["data"]["model"].clone())
        };
        assert_eq!(model("implement"), Some("sonnet".into()));
        assert_eq!(model("review"), Some("opus".into()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A repository that names its test command is verified by Orteca running
    /// it: no Verify call. A failure is followed by a Fix, which is told what the
    /// suite printed, and the suite runs again on the fix.
    #[tokio::test]
    async fn a_declared_test_command_is_verified_without_an_agent_call() {
        let store = Store::in_memory().unwrap();
        let script = |code: u8| {
            format!(
                r#"{{"scripts":{{"test":"node -e \"console.log('header is not bold');process.exit({code})\""}}}}"#
            )
        };
        let passing = serde_json::json!({"checks": [{"command": "npm test", "passed": true, "output": "ok"}], "verdict": "pass"});

        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "local-verify-pass", route.clone());
        let dir = request.dir.clone();
        std::fs::write(dir.join("package.json"), script(0)).unwrap();
        claude_shim(&mut request, &passing);
        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;
        assert_eq!(result.status, "done", "{:?}", result.budget_stop);
        assert_eq!(result.calls_used, 1, "an agent was asked to run the tests");
        let verify = result.stages.last().unwrap();
        assert_eq!(
            (
                verify.stage,
                verify
                    .artifact
                    .as_ref()
                    .map(|a| a["checks"][0]["command"].clone())
            ),
            (Stage::Verify, Some("npm test".into()))
        );
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        assert!(
            project::check_commands(&dir).is_empty(),
            "npm init's placeholder is not a test command"
        );
        std::fs::remove_dir_all(dir).unwrap();

        let mut request = routed(&store, "local-verify-fail", route);
        let dir = request.dir.clone();
        std::fs::write(dir.join("package.json"), FAILS_ONCE).unwrap();
        claude_shim(&mut request, &passing);
        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;
        assert_eq!(result.status, "done");
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Verify, Stage::Fix, Stage::Verify],
            "the fix was not checked again"
        );
        assert_eq!(
            result.calls_used, 2,
            "only Implement and Fix are agent calls"
        );
        let calls = briefs(&dir);
        assert!(
            calls[1].contains("header is not bold"),
            "the fix was not told what failed: {}",
            calls[1]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_missing_test_dependency_never_buys_an_agent_fix() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "missing-test-program", route);
        let dir = request.dir.clone();
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"test":"node -e \"console.log('mysql is not recognized as an internal or external command');process.exit(1)\""}}"#,
        )
        .unwrap();
        let passing = serde_json::json!({"checks": [{"command": "npm test", "passed": true, "output": "ok"}], "verdict": "pass"});
        claude_shim(&mut request, &passing);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "failed");
        assert_eq!(result.calls_used, 1, "an environment failure bought a Fix");
        assert!(
            result
                .failure
                .as_deref()
                .is_some_and(|message| message.contains("required program is missing"))
        );
        assert_eq!(
            result.stages.iter().map(|note| note.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Verify]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Guarded work in a repository with a test command runs the tests before
    /// the deep Review. Passing work is reviewed knowing the tests pass; a
    /// failure is fixed and tested again, and the Review reads the fix.
    #[tokio::test]
    async fn guarded_work_runs_its_tests_before_the_review() {
        let store = Store::in_memory().unwrap();
        let script =
            |code: u8| format!(r#"{{"scripts":{{"test":"node -e \"process.exit({code})\""}}}}"#);
        let review = serde_json::json!({"findings": [], "verdict": "pass"});
        let fixed = serde_json::json!({"checks": [{"command": "npm test", "passed": true, "output": "ok"}], "verdict": "pass"});
        let signals = RepoSignals {
            checks_locally: true,
            ..RepoSignals::default()
        };
        let route = routing::route(
            "add an authorization check before the delete endpoint",
            Mode::Balanced,
            &signals,
        );
        let stages =
            |result: &TaskResult| result.stages.iter().map(|n| n.stage).collect::<Vec<_>>();

        let mut request = routed(&store, "guarded-tests-pass", route.clone());
        let dir = request.dir.clone();
        std::fs::write(dir.join("package.json"), script(0)).unwrap();
        claude_shim(&mut request, &review);
        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;
        assert_eq!(result.status, "done", "{:?}", result.budget_stop);
        assert_eq!(
            stages(&result),
            [Stage::Implement, Stage::Verify, Stage::Review]
        );
        assert_eq!(result.calls_used, 2);
        assert!(
            briefs(&dir)[1].contains("tests already pass"),
            "the review was not told the tests pass"
        );
        std::fs::remove_dir_all(dir).unwrap();

        let mut request = routed(&store, "guarded-tests-fail", route);
        let dir = request.dir.clone();
        std::fs::write(dir.join("package.json"), FAILS_ONCE).unwrap();
        claude_shim_answers(&mut request, &[review.clone(), fixed, review]);
        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;
        assert_eq!(result.status, "done", "{:?}", result.budget_stop);
        assert_eq!(
            stages(&result),
            [
                Stage::Implement,
                Stage::Verify,
                Stage::Fix,
                Stage::Verify,
                Stage::Review
            ]
        );
        assert_eq!(result.calls_used, 3);
        let calls = briefs(&dir);
        assert!(
            calls[1].contains("runs again after this call"),
            "{}",
            calls[1]
        );
        assert!(
            calls[2].contains("tests already pass"),
            "the review was not told the fix passed its tests"
        );
        assert!(
            calls[2].contains("The change under review"),
            "the review was not handed the patch"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn codex_schema_output_becomes_a_valid_stage_artifact() {
        let store = Store::in_memory().unwrap();
        let mut route = one_call("make a plan");
        route.stages = vec![Stage::Plan];
        let request = routed(&store, "codex-artifact", route);
        let dir = request.dir.clone();
        let artifact = serde_json::json!({
            "objective": "rename it",
            "constraints": [],
            "affected_areas": [],
            "implementation_steps": [],
            "risks": [],
            "tests_required": []
        });
        codex_artifact_shim(&request, &artifact);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "done");
        assert_eq!(result.stages.len(), 1);
        assert_eq!(result.stages[0].artifact, Some(artifact));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Milestone 5's promise, kept across a route: an instruction given during
    /// one stage reaches every stage after it, and is not delivered twice by
    /// resuming the stage it arrived in.
    #[tokio::test]
    async fn an_instruction_given_in_one_stage_carries_into_the_next() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "redesign the storage subsystem",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let request = routed(&store, "carried", route);
        let (dir, task) = (request.dir.clone(), request.task_id);
        answering_shim(&request, 1);

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| {
                let _ = heard.send(e.kind());
                Ok(())
            }),
            async {
                // Once the first stage is genuinely talking, so this is an
                // instruction to a running agent and not a race with start-up.
                wait_for_kind(&mut hearing, "text").await;
                let _ = live
                    .instruct(task, "never touch the public API".into(), false)
                    .await;
            }
        );

        // The shim echoes its brief, which is no Verify artifact, and changes
        // nothing, so its Fix makes no progress and the route ends there.
        assert_eq!(result.status, "verifyFailed");
        // One call per stage: the instruction did not buy an extra process to
        // say the same thing twice.
        assert_eq!(result.calls_used as usize, result.stages.len());
        let calls = briefs(&dir);
        assert!(
            calls[1..]
                .iter()
                .all(|b| b.contains("never touch the public API")),
            "a later stage lost the instruction: {calls:?}"
        );
        assert!(
            calls[1..]
                .iter()
                .all(|b| b.contains("Standing instructions")),
            "the instruction was not carried as a standing constraint"
        );
        // And it is in the log whatever became of it.
        assert!(store.event_kinds(task).contains(&"instruction".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A review that asks for changes is followed by one Fix. A Fix that
    /// changes nothing stops before Verify, with the findings kept.
    #[tokio::test]
    async fn a_rejected_review_whose_fix_changes_nothing_ends_the_route() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "add an authorization check before the delete endpoint",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "review", route);
        let (dir, task) = (request.dir.clone(), request.task_id);
        // Every call reports a Review artifact asking for changes. Only the
        // Review stage is contracted for one, so only there does it count -
        // the same JSON is not a plan just because a Plan stage returned it.
        claude_shim(
            &mut request,
            &serde_json::json!({
                "findings": [{"severity": "high", "file": "a.rs", "line": 1, "issue": "unchecked", "fix": "check it"}],
                "verdict": "changes_requested"
            }),
        );

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "reviewRejected");
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Review, Stage::Fix]
        );
        assert_eq!(result.calls_used, 3);
        let artifact = result.stages[1]
            .artifact
            .as_ref()
            .expect("the review artifact was dropped");
        assert_eq!(artifact["verdict"], "changes_requested");
        let stop = result
            .budget_stop
            .as_ref()
            .expect("review stop was not returned");
        assert_eq!(stop.limit, "review");
        assert_eq!(stop.remaining, [Stage::Verify]);
        assert!(stop.message.contains("changed nothing"), "{}", stop.message);
        // Recorded, so the findings are not only on screen.
        let logged = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["kind"] == "artifact" && v["data"]["stage"] == "review");
        assert_eq!(logged.expect("no artifact row")["data"]["valid"], true);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A Verify that ends cleanly while one of its checks failed is not `done`,
    /// whatever verdict it gave itself. It is fixed on the same tier and checked
    /// again; when the Fix changes nothing the route ends as `verifyFailed`.
    #[tokio::test]
    async fn checks_a_fix_cannot_move_end_the_route_as_verify_failed() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        assert_eq!(route.stages, [Stage::Implement, Stage::Verify]);
        let mut request = routed(&store, "verify-failed", route);
        let (dir, task) = (request.dir.clone(), request.task_id);
        claude_shim(
            &mut request,
            &serde_json::json!({
                "checks": [{"command": "npm test", "passed": false, "output": "1 failing"}],
                "verdict": "pass"
            }),
        );

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "verifyFailed");
        assert_eq!(result.calls_used, 4);
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Verify, Stage::Fix, Stage::Verify]
        );
        assert_eq!(
            result.failure, None,
            "failing checks are a stop, not a provider fault"
        );
        let stop = result
            .budget_stop
            .as_ref()
            .expect("verify stop was not returned");
        assert_eq!(stop.limit, "verify");
        assert!(stop.remaining.is_empty());
        let payloads = store.event_payloads(task);
        let model = |stage: &str| {
            payloads
                .iter()
                .find(|v| v["kind"] == "stage" && v["data"]["stage"] == stage)
                .map(|v| v["data"]["model"].clone())
        };
        assert_eq!(
            model("fix"),
            model("implement"),
            "the fix left the model that wrote the change"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_failed_local_verify_speaks_for_itself() {
        let artifact = serde_json::json!({
            "checks": [
                {"command": "composer test", "passed": false, "output": "Class X cannot be found\n"},
                {"command": "npm test", "passed": true, "output": "ok"}
            ],
            "verdict": "fail"
        });
        assert_eq!(
            failed_summary(Some(&artifact)),
            "`composer test` did not pass.\n\n```\nClass X cannot be found\n```"
        );
        assert_eq!(failed_summary(None), "");
    }

    #[tokio::test]
    async fn a_changed_fix_gets_no_second_paid_attempt_when_verify_still_fails() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "one-fix", route);
        let dir = request.dir.clone();
        let failing = serde_json::json!({
            "checks": [{"command": "npm test", "passed": false, "output": "still failing"}],
            "verdict": "fail"
        });
        claude_shim(&mut request, &failing);
        let shim = dir.join("fake.js");
        let script = std::fs::read_to_string(&shim).unwrap().replacen(
            "fs.appendFileSync(",
            "if(n===2)fs.writeFileSync('changed.txt','changed');fs.appendFileSync(",
            1,
        );
        std::fs::write(&shim, script).unwrap();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "verifyFailed");
        assert_eq!(
            result.stages.iter().map(|note| note.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Verify, Stage::Fix, Stage::Verify]
        );
        assert_eq!(result.calls_used, 4, "a second Fix call was started");
        let stop = result.budget_stop.expect("the retry limit was not returned");
        assert_eq!((stop.limit, stop.allowed, stop.observed), ("verify", 1, 1));
        assert!(stop.message.contains("One automatic fix"), "{}", stop.message);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A Fix rescues the task: checks that failed pass when they run again on
    /// the fix, and the run is done.
    #[tokio::test]
    async fn a_fix_that_passes_its_check_again_finishes_the_task() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "escalated-pass", route);
        let dir = request.dir.clone();
        let failing = serde_json::json!({"checks": [{"command": "npm test", "passed": false, "output": "header is not bold"}], "verdict": "fail"});
        let passing = serde_json::json!({"checks": [{"command": "npm test", "passed": true, "output": "ok"}], "verdict": "pass"});
        claude_shim_answers(&mut request, &[failing.clone(), failing, passing]);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "done");
        assert_eq!(result.calls_used, 4);
        assert!(result.budget_stop.is_none());
        let calls = briefs(&dir);
        assert!(
            calls[2].contains("did not pass") && calls[2].contains("header is not bold"),
            "the fix was not told what failed: {}",
            calls[2]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A review runs once. Its findings are fixed, then the existing Verify
    /// stage checks the result without buying another independent review.
    #[tokio::test]
    async fn a_rejected_review_is_fixed_then_verified() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "add an authorization check before the delete endpoint",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "review-again", route);
        let dir = request.dir.clone();
        let rejected = serde_json::json!({
            "findings": [{"severity": "high", "file": "a.rs", "line": 1, "issue": "unchecked", "fix": "check it"}],
            "verdict": "changes_requested"
        });
        // Valid as a Fix and Verify, since the shim gives both the same answer.
        let passing = serde_json::json!({"findings": [], "checks": [{"command": "cargo test auth", "passed": true, "output": "ok"}], "verdict": "pass"});
        claude_shim_answers(&mut request, &[rejected.clone(), rejected, passing]);
        let shim = dir.join("fake.js");
        let script = std::fs::read_to_string(&shim).unwrap().replacen(
            "fs.appendFileSync(",
            "if(n===2)fs.writeFileSync('fixed.txt','fixed');fs.appendFileSync(",
            1,
        );
        std::fs::write(&shim, script).unwrap();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "done", "{:?}", result.budget_stop);
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [
                Stage::Implement,
                Stage::Review,
                Stage::Fix,
                Stage::Verify
            ]
        );
        assert_eq!(result.calls_used, 4);
        let calls = briefs(&dir);
        assert!(
            calls[2].contains("Validated review artifact") && calls[2].contains("unchecked"),
            "the fix was not told what the review found: {}",
            calls[2]
        );
        assert!(calls[1].contains("The change under review"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_missing_review_artifact_is_not_a_pass() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "add an authorization check before the delete endpoint",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "missing-review", route);
        let dir = request.dir.clone();
        claude_shim(
            &mut request,
            &serde_json::json!({"objective": "not a review"}),
        );

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "reviewRejected");
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Review, Stage::Fix]
        );
        assert!(result.stages[1].artifact.is_none());
        assert_eq!(result.budget_stop.as_ref().map(|s| s.limit), Some("review"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A Codex Fix resumes the session that wrote the change, on the same
    /// model, instead of a fresh process that reads everything again.
    #[tokio::test]
    async fn a_codex_fix_resumes_the_session_that_wrote_the_change() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let request = routed(&store, "fix-resumes", route);
        let dir = request.dir.clone();
        std::fs::write(dir.join("package.json"), FAILS_ONCE).unwrap();
        std::fs::write(
            &request.program,
            "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n",
        )
        .unwrap();
        let log = dir.join("argv.log").to_string_lossy().replace('\\', "/");
        std::fs::write(
            dir.join("fake.js"),
            format!(
                "const fs=require('fs');const a=process.argv.slice(2);let i='';\
                 process.stdin.setEncoding('utf8');process.stdin.on('data',d=>i+=d);\
                 process.stdin.on('end',()=>{{fs.appendFileSync('{log}',a.join(' ')+'\\n');\
                 if(a.includes('resume')){{console.log(JSON.stringify({{type:'item.completed',item:{{type:'agent_message',\
                 text:JSON.stringify({{checks:[{{command:'npm test',passed:true,output:'ok'}}],verdict:'pass'}})}}}}));}}\
                 else{{console.log(JSON.stringify({{type:'thread.started',thread_id:'sess-1'}}));\
                 console.log(JSON.stringify({{type:'item.completed',item:{{type:'agent_message',text:'edited'}}}}));}}\
                 console.log(JSON.stringify({{type:'turn.completed',usage:{{input_tokens:1,output_tokens:1}}}}));}});"
            ),
        )
        .unwrap();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(
            result.status, "done",
            "{:?} {:?}",
            result.failure, result.budget_stop
        );
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Verify, Stage::Fix, Stage::Verify]
        );
        let calls: Vec<String> = std::fs::read_to_string(dir.join("argv.log"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(!calls[0].contains("resume"), "{}", calls[0]);
        assert!(calls[1].starts_with("exec resume sess-1"), "{}", calls[1]);
        assert!(
            calls[1].contains("gpt-5.6-terra"),
            "the fix left the model that wrote the change: {}",
            calls[1]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Claude's Fix continues the session that wrote the change.
    #[tokio::test]
    async fn a_claude_fix_resumes_the_session_that_wrote_the_change() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "make the header bold",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "claude-fix-resumes", route);
        request.id = ProviderId::Claude;
        let dir = request.dir.clone();
        std::fs::write(dir.join("package.json"), FAILS_ONCE).unwrap();
        std::fs::write(
            &request.program,
            "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n",
        )
        .unwrap();
        let log = dir.join("argv.log").to_string_lossy().replace('\\', "/");
        std::fs::write(
            dir.join("fake.js"),
            format!(
                "const fs=require('fs');fs.appendFileSync('{log}',process.argv.slice(2).join(' ')+'\\n');\
                 let answered=false;process.stdin.setEncoding('utf8');process.stdin.on('data',()=>{{if(answered)return;answered=true;\
                 console.log(JSON.stringify({{type:'system',subtype:'init',session_id:'claude-1'}}));\
                 console.log(JSON.stringify({{type:'result',subtype:'success',result:'ok',\
                 structured_output:{{checks:[{{command:'npm test',passed:true,output:'ok'}}],verdict:'pass'}},\
                 usage:{{input_tokens:1,output_tokens:1}},total_cost_usd:0.01}}));}});\
                 process.stdin.on('end',()=>process.exit(0));"
            ),
        )
        .unwrap();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(
            result.status, "done",
            "{:?} {:?}",
            result.failure, result.budget_stop
        );
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Implement, Stage::Verify, Stage::Fix, Stage::Verify]
        );
        let calls: Vec<String> = std::fs::read_to_string(dir.join("argv.log"))
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(!calls[0].contains("--resume"), "{}", calls[0]);
        assert!(calls[1].starts_with("-p "), "{}", calls[1]);
        assert!(calls[1].ends_with("--resume claude-1"), "{}", calls[1]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A follow-up resumes the session it was handed and sends only the reply,
    /// unless the first stage now asks for another model.
    #[tokio::test]
    async fn a_follow_up_resumes_on_the_same_model_and_sends_only_the_reply() {
        let store = Store::in_memory().unwrap();
        let fake_claude = |request: &mut Request| {
            request.id = ProviderId::Claude;
            let dir = request.dir.clone();
            std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n").unwrap();
            let log = dir.join("argv.log").to_string_lossy().replace('\\', "/");
            std::fs::write(
                dir.join("fake.js"),
                format!(
                    "const fs=require('fs');process.stdin.setEncoding('utf8');process.stdin.once('data',d=>{{\
                     fs.appendFileSync('{log}',process.argv.slice(2).join(' ')+'\\n'+d+'\\n');\
                     console.log(JSON.stringify({{type:'system',subtype:'init',session_id:'claude-2'}}));\
                     console.log(JSON.stringify({{type:'result',subtype:'success',result:'which table?',\
                     usage:{{input_tokens:1,output_tokens:1}},total_cost_usd:0.01}}));}});\
                     process.stdin.on('end',()=>process.exit(0));"
                ),
            )
            .unwrap();
            dir
        };

        let mut first = routed(&store, "follow-up-1", one_call("fix the typo in the readme"));
        let dir = fake_claude(&mut first);
        let point = stream(&store, &Live::default(), first, |_| Ok(())).await.resume.unwrap();
        assert_eq!(point.session, "claude-2");
        std::fs::remove_dir_all(&dir).unwrap();

        let mut same = routed(&store, "follow-up-2", one_call("fix the typo in the readme"));
        let dir = fake_claude(&mut same);
        same.resume = Some(Resume { session: "claude-1".into(), reply: "the quotes table".into(), ..point.clone() });
        same.prompt = "fix the typo".into();
        stream(&store, &Live::default(), same, |_| Ok(())).await;
        let log = std::fs::read_to_string(dir.join("argv.log")).unwrap();
        assert!(log.contains("--resume claude-1"), "{log}");
        assert!(log.contains("the quotes table"), "{log}");
        assert!(!log.contains("fix the typo"), "the resumed session already has the request: {log}");
        std::fs::remove_dir_all(&dir).unwrap();

        let mut other = routed(&store, "follow-up-3", one_call("fix the typo in the readme"));
        let dir = fake_claude(&mut other);
        other.prompt = "fix the typo".into();
        other.resume = Some(Resume { model: "some-other-model".into(), reply: "x".into(), ..point });
        stream(&store, &Live::default(), other, |_| Ok(())).await;
        let log = std::fs::read_to_string(dir.join("argv.log")).unwrap();
        assert!(!log.contains("--resume"), "{log}");
        assert!(log.contains("fix the typo"), "{log}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Structured output that does not match the contract is not an artifact,
    /// and its prose is never read in its place.
    #[tokio::test]
    async fn a_malformed_artifact_is_recorded_as_missing_and_never_parsed_from_prose() {
        let store = Store::in_memory().unwrap();
        let mut route = one_call("fix the typo in the readme");
        // One Plan stage, so there is a contract to fail.
        route.stages = vec![Stage::Plan];
        let mut request = routed(&store, "artifact", route);
        let (dir, task) = (request.dir.clone(), request.task_id);
        // A plan missing every required list, beside prose that reads like one.
        claude_shim(
            &mut request,
            &serde_json::json!({"objective": "half a plan"}),
        );

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        let note = result.stages.first().expect("the stage left no note");
        assert!(
            note.artifact.is_none(),
            "a half-built plan was accepted as a plan"
        );
        // The words are kept as words, to be forwarded verbatim and labelled
        // unvalidated. They are never mined for the fields the schema would
        // have filled.
        assert_eq!(note.summary, "stage answered");
        let logged = store
            .event_payloads(task)
            .into_iter()
            .find(|v| v["kind"] == "artifact")
            .expect("no artifact row");
        assert_eq!(
            logged["data"]["valid"], false,
            "a missing artifact must be recorded as missing"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn codex_summary_falls_back_to_its_last_message() {
        let outcome = replay(ProviderId::Codex);
        assert!(outcome.done);
        assert!(outcome.failure.is_none());
        // Codex has no final-answer field, so Done.result is empty.
        assert!(outcome.result.is_empty());
        assert_eq!(outcome.summary(), outcome.last_text);
        assert!(!outcome.summary().is_empty());
    }

    /// A real two-turn Claude session, captured from the CLI while verifying
    /// the streaming-input shape. It is the only recording of a steered run,
    /// and it is what checks the token arithmetic against the provider instead
    /// of against a guess: two results, tokens that add, a cost that does not.
    /// Only the machine paths in it were normalised, as in `claude-run.jsonl`.
    #[test]
    fn a_recorded_steered_run_counts_every_turn_once() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("claude-steered-run.jsonl");
        let mut outcome = Outcome::default();
        let mut results = 0;
        for event in mock::replay(ProviderId::Claude, &path).expect("fixture") {
            if matches!(event, ProviderEvent::Done { .. }) {
                results += 1;
            }
            outcome.absorb(&event);
        }

        assert_eq!(results, 2, "a steered run reports one result per turn");
        assert_eq!(outcome.summary(), "SECOND", "the last turn is the answer");
        let usage = outcome.usage.expect("usage");
        // 4 + 6 output, and the cache-creation tokens fold into input as
        // claude.rs already does: (2 + 5321) + (2 + 58).
        assert_eq!(usage.output_tokens, 10);
        assert_eq!(usage.input_tokens, 5383);
        assert_eq!(usage.cached_input_tokens, 44451 + 49772);
        // The CLI's own running total, not 0.0302182 + 0.0404686.
        assert_eq!(usage.cost_usd, Some(0.0404686));
    }

    #[test]
    fn claude_summary_is_its_own_result() {
        let outcome = replay(ProviderId::Claude);
        assert_eq!(
            outcome.summary(),
            "The index already orders recents by opened_seq."
        );
        assert_eq!(
            outcome.usage.as_ref().map(|u| u.cost_quality),
            Some(CostQuality::Estimated)
        );
    }

    #[test]
    fn a_run_that_dies_early_has_no_token_count_at_all() {
        let mut outcome = Outcome::default();
        outcome.absorb(&ProviderEvent::Text("half an answer".into()));
        assert!(
            outcome.usage.is_none(),
            "a zero here would read as a free run"
        );
        assert!(!outcome.done);
    }

    #[test]
    fn an_exit_without_a_result_is_a_failure_that_says_why() {
        let message = exit_message(ProviderId::Codex, Some(1), &["boom".into()]);
        assert!(message.contains("exited with code 1"));
        assert!(message.contains("boom"));
        // Cancel and crash read differently on purpose.
        assert!(exit_message(ProviderId::Claude, None, &[]).contains("stopped"));
    }

    #[test]
    fn an_empty_prompt_never_reaches_a_cli() {
        assert_eq!(clean_prompt("  \n "), None);
        assert_eq!(clean_prompt(" build it "), Some("build it".into()));
    }
}
