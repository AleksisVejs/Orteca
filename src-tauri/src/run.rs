//! Running a route: prompt -> stages -> streams -> diff -> result.
//!
//! The route and its ceilings are decided in `routing` before anything here
//! starts a process. This module's job is to honour them: run each stage in
//! turn, normalise every line through the provider's parser, write each event
//! to the append-only log, let the user steer and stop it, and end with a diff
//! they can check.
//!
//! A trivial task's route is one Implement stage, so that path is exactly the
//! single-stage run Milestone 4 shipped, with a budget attached.
//!
//! What this module will not do is spend more than the route declared. When a
//! ceiling is reached the run stops with `budgetReached`, keeping the work, the
//! diff and the usage - and it never promotes itself to a longer route to
//! finish the job. Spending more is the user's decision to make.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use crate::error::{AppError, ErrorKind};
use crate::proc::{self, Line};
use crate::project::{self, FileStat};
use crate::providers::{claude, FailureKind, ProviderEvent, ProviderId, Steering, Usage};
use crate::routing::{self, Route, Stage, StageNote};
use crate::store::{Baseline, Store};

// Commands the agent may never run, in either shell. Deny beats allow, so these
// hold even though `--allowedTools` grants Bash and PowerShell outright.
// Scoped rules match command spelling, not every way of executing Git: this is a
// guardrail against an agent going wrong, not a sandbox against a hostile one.
// Claude exposes no OS-level sandbox flag the way `codex --sandbox` does.
const CLAUDE_DENY_COMMANDS: &[&str] = &[
    // Rewriting or publishing the user's history. Orteca reads git, never rewrites it.
    "git push:*", "git reset:*", "git clean:*", "git rebase:*",
    "git restore:*", "git checkout --:*", "git filter-branch:*",
    // Destroying files outside a normal edit.
    "rm:*", "rmdir:*", "del:*", "rd:*", "Remove-Item:*",
    // Publishing under the user's name.
    "npm publish:*", "cargo publish:*", "gh release:*",
    // Reaching the network, which is how a bad instruction exfiltrates a repo.
    "curl:*", "wget:*", "Invoke-WebRequest:*", "Invoke-RestMethod:*", "scp:*", "ssh:*",
    // Touching the machine rather than the project.
    "shutdown:*", "reg:*", "schtasks:*", "net user:*", "Set-ExecutionPolicy:*",
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
fn claude_allowed(stage: Stage) -> Vec<String> {
    if stage.writes() {
        return ["Bash", "PowerShell"].into_iter().map(str::to_string).collect();
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
        tools.extend([
            "Bash(npm test *)",
            "Bash(npm run build *)",
            "Bash(node --test *)",
            "Bash(cargo test *)",
            "Bash(cargo clippy *)",
        ].into_iter().map(str::to_string));
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
        self.0.lock().expect("live runs poisoned").insert(task_id, tx);
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
    /// Which ceiling or explicit stage outcome: `calls`, `turns`, `tokens` or
    /// `review`.
    pub limit: &'static str,
    pub allowed: u64,
    pub observed: u64,
    /// The stages the route still had. Starting them is a deliberate choice the
    /// user makes; nothing here does it for them.
    pub remaining: Vec<Stage>,
    pub message: String,
}

/// What the UI gets when the run ends.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub task_id: i64,
    /// `done`, `cancelled`, `failed`, `budgetReached` or `reviewRejected`.
    pub status: &'static str,
    /// The provider's final answer, or its last message if it reports no final
    /// field. Empty is possible and is not an error.
    pub summary: String,
    pub failure: Option<String>,
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
    /// The route this run was given before any provider started, ceilings and
    /// classifier signals included.
    pub route: Route,
    /// Stages that actually ran, in order, with any validated artifact.
    pub stages: Vec<StageNote>,
    /// Provider processes started, stages and resumes together. With
    /// `route.budget.max_agent_calls` this is the "calls avoided" figure, and
    /// it is exact: Orteca chose the route and knows the ceiling.
    pub calls_used: u32,
    /// Provider turns that completed. Exact for the same reason.
    pub turns_used: u32,
    /// Set when a ceiling or review outcome stopped the run.
    pub budget_stop: Option<BudgetStop>,
    /// Comparable finished runs in this project, once there are five. The UI
    /// shows no savings figure without it.
    pub baseline: Option<Baseline>,
}

/// What one stage asks of its CLI, beyond the prompt.
#[derive(Debug, Clone)]
pub struct StagePlan {
    pub stage: Stage,
    /// The turn ceiling this process may use. Passed to Claude, which has a
    /// flag for it; enforced by Orteca counting turns for Codex, which has not.
    pub max_turns: Option<u32>,
    /// The artifact contract, already written to a file. Claude takes the
    /// schema text inline, Codex takes the path.
    pub schema: Option<PathBuf>,
}

/// The argv for one stage.
///
/// Claude is never given `--bare`: that would disable OAuth and force an API
/// key, billing the user instead of using the subscription they already have.
/// Codex is never given `--sandbox danger-full-access`; `workspace-write` is
/// the widest access Orteca asks for, and a stage that is not meant to edit
/// gets `read-only` instead.
///
/// Both budget and schema flags were checked against the installed CLIs on
/// 2026-09-12 rather than taken from the spec. `claude --max-turns <turns>`
/// exists and is accepted, though it is missing from `--help`; `codex exec`
/// has no turn flag at all, which is why Orteca counts turns itself.
pub fn args(id: ProviderId, plan: &StagePlan) -> Vec<String> {
    let arg = str::to_string;
    let writes = plan.stage.writes();
    match id {
        ProviderId::Codex => [
            vec![
                arg("exec"),
                arg("-"),
                arg("--json"),
                arg("--sandbox"),
                // A stage with no business editing cannot edit. Codex has an
                // OS-level fence for this; using it is cheaper and more certain
                // than asking the agent nicely.
                arg(if writes { "workspace-write" } else { "read-only" }),
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
            args.extend(claude_allowed(plan.stage));
            // The denylist narrows the write-stage shell grant and removes
            // native edit tools from every non-writing stage.
            args.push(arg("--disallowedTools"));
            args.extend(claude_deny(writes));
            args.extend(turn_limit(plan));
            args.extend(schema_arg(plan));
            args
        }
    }
}

/// Claude's own turn ceiling. Absent from `--help`, present in the parser, and
/// confirmed by asking the CLI for it without a value: it answers `option
/// '--max-turns <turns>' argument missing`, which an unknown flag does not.
fn turn_limit(plan: &StagePlan) -> Vec<String> {
    plan.max_turns
        .map_or_else(Vec::new, |turns| vec!["--max-turns".to_string(), turns.to_string()])
}

/// Claude takes the schema as text on the command line, not as a path.
fn schema_arg(plan: &StagePlan) -> Vec<String> {
    plan.stage
        .schema()
        .filter(|_| plan.schema.is_some())
        .map_or_else(Vec::new, |schema| vec!["--json-schema".to_string(), schema.to_string()])
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
    failure: Option<String>,
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
            ProviderEvent::Failed { message, .. } => self.failure = Some(message.clone()),
            _ => {}
        }
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
                dir.join(format!("task-{task_id}-{}-{}.jsonl", stage.name(), id.program()))
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
        if std::io::Write::write_all(file, format!("{value}
").as_bytes()).is_err() {
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
        Launch {
            argv: id.resume_args(session, plan.schema.as_deref()),
            opening: held.join("\n"),
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
    /// The current stage plus the stages after it. Used to tell the user what
    /// a turn ceiling left unstarted.
    remaining: Vec<Stage>,
    /// The route's cumulative token ceiling, checked after every turn.
    max_reported_tokens: Option<u64>,
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
    /// Completed turns across the whole task, and within this process. The
    /// second is what a turn ceiling is measured against, because that is what
    /// `claude --max-turns` counts and what Codex would count if it could.
    turns_used: u32,
    turns_this_call: u32,
    /// Every instruction the user has given, in order. Unlike `held` this is
    /// never cleared: each later stage's brief repeats all of them, so an
    /// instruction never silently expires.
    constraints: Vec<String>,
    /// What each finished stage handed on.
    notes: Vec<StageNote>,
    /// The structured artifact this stage returned, before it is validated.
    structured: Option<serde_json::Value>,
    /// Set when a ceiling ended the run. No later stage starts after this.
    budget_stop: Option<BudgetStop>,
    /// The route is finished early, and not because anything went wrong: a
    /// review asked for changes, so the remaining stages have nothing useful
    /// left to do. What happens next is the user's call.
    halt: bool,
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
}

impl State {
    /// Cumulative uncached tokens: what the inter-turn guard is measured
    /// against. Cache reads are left out, as the baseline leaves them out: the
    /// CLI re-reads its own ~40-55k context every turn, so counting it put a
    /// finished trivial run past its ceiling. A run whose provider reported
    /// nothing has no honest number and therefore cannot have crossed a ceiling.
    fn reported_tokens(&self) -> u64 {
        self.outcome
            .usage
            .as_ref()
            .map_or(0, |u| u.input_tokens.saturating_add(u.output_tokens))
    }
}

/// What happens after one process ends.
enum Next {
    /// The task is over, however it ended.
    Ended,
    /// The user asked for an instruction to apply now, so the recorded session
    /// is picked back up carrying it.
    Restart(Launch),
}

pub async fn stream(store: &Store, live: &Live, request: Request, emit: impl Fn(&ProviderEvent) -> crate::error::Result<()>) -> TaskResult {
    let started_at = std::time::Instant::now();
    let Request { task_id, id, program, dir, prompt, route, base_commit, dirty_at_start, before_run, recordings } = request;
    // Registered before the CLI is even spawned: a run is stoppable from the
    // moment the user can see it, including while a slow Node shim starts up.
    let mut control = live.open(task_id);
    let mut ctx = Context {
        task_id,
        id,
        program,
        dir,
        plan: StagePlan { stage: Stage::Implement, max_turns: None, schema: None },
        final_stage: true,
        remaining: Vec::new(),
        max_reported_tokens: route.budget.max_reported_tokens,
    };
    let mut state = State {
        outcome: Outcome::default(),
        recording: Recording::new(None, task_id, Stage::Implement, id),
        calls_used: 0,
        turns_used: 0,
        turns_this_call: 0,
        constraints: Vec::new(),
        notes: Vec::new(),
        structured: None,
        budget_stop: None,
        halt: false,
        noise: Vec::new(),
        unknown_events: 0,
        held: Vec::new(),
        session: None,
        apply_now_pending: false,
    };

    // The whole decision, recorded before a single process starts. Without this
    // row a later milestone can see what a run cost but not what it was allowed
    // to cost, and cannot tell a good route from a lucky one.
    if let Err(e) = note(store, &ctx, "routing", &routing_payload(&route)) {
        state.outcome.failure = Some(format!("could not record the route: {}", e.message));
    }

    let stages = route.stages.clone();
    for (index, stage) in stages.iter().copied().enumerate() {
        if state.outcome.failure.is_some()
            || state.outcome.cancelled
            || state.halt
            || state.budget_stop.is_some()
        {
            break;
        }
        // Checked before the stage, never during it. Orteca cannot interrupt a
        // turn that is already running, so the honest place to enforce a
        // ceiling is the gap between one call and the next.
        if let Some(stop) = gate(&route, &state, index, &stages) {
            let _ = note(store, &ctx, "budget", &budget_payload("stopped", &stop));
            state.budget_stop = Some(stop);
            break;
        }

        ctx.plan = StagePlan {
            stage,
            max_turns: route.budget.max_turns,
            schema: write_schema(task_id, stage),
        };
        ctx.final_stage = index + 1 == stages.len();
        ctx.remaining = stages[index..].to_vec();
        state.outcome.begin_stage();
        state.structured = None;
        state.session = None;
        state.turns_this_call = 0;
        state.recording = Recording::new(recordings.as_deref(), task_id, stage, id);

        let brief = routing::brief(&route, stage, &prompt, &state.constraints, &state.notes);
        if let Err(e) = note(store, &ctx, "stage", &stage_payload(stage, index, &route)) {
            state.outcome.failure = Some(format!("could not record the stage: {}", e.message));
            break;
        }

        // Usually one pass. A checkpoint provider told to apply an instruction
        // now ends its process and comes back through here resuming its own
        // session.
        let mut launch = Launch::first(id, &brief, &ctx.plan);
        loop {
            match attempt(store, &ctx, &mut state, &mut control, &emit, launch).await {
                Next::Ended => break,
                Next::Restart(again) => launch = again,
            }
        }

        // A stage that ended in an instruction still waiting is not a stage
        // that lost it: the constraint list carries it into the next brief.
        state.held.clear();
        let note_for_stage = finish_stage(store, &ctx, &mut state, stage);
        state.notes.push(note_for_stage);
    }

    let dir = ctx.dir;
    let mut outcome = state.outcome;

    live.close(task_id);

    // A run that never reported usage has no honest number; this writes the
    // `unavailable` row rather than leaving the task looking free.
    if let Err(e) = store.record_usage(task_id, None, id.program(), outcome.usage.as_ref()) {
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
    // Codex on Windows can run `workspace-write` as read-only and still end its
    // turn cleanly. Every command is rejected inside the CLI, and `--json`
    // carries none of it - the rejections exist only in Codex's own rollout.
    // What the stream cannot hide is the repository: a writing stage that left
    // no change of its own did not do what `done` would claim. Prose is not read.
    if id == ProviderId::Codex
        && outcome.failure.is_none()
        && !outcome.cancelled
        && state.budget_stop.is_none()
        && state.notes.iter().any(|n| n.stage.writes())
        && diff.iter().all(|f| f.origin == Some(project::Origin::BeforeRun))
    {
        outcome.failure = Some(
            "Codex ended its implement stage without changing any file. A sandbox that fell back to read-only looks exactly like this."
                .into(),
        );
    }
    let duration_ms = started_at.elapsed().as_millis().min(u64::MAX as u128) as u64;
    if let Some(message) = &outcome.failure {
        let event = ProviderEvent::Failed { kind: crate::providers::classify_failure(message), message: message.clone() };
        if let Err(e) = record(store, task_id, "run", id, &event) {
            outcome.failure = Some(format!("{message}; could not log failure: {}", e.message));
        }
    }
    // A budget stop is its own outcome. It is not a failure - nothing went
    // wrong - and not a success, because the route did not finish. The work,
    // the diff and the usage are all kept exactly as they are.
    let mut status = if state.budget_stop.is_some() && outcome.failure.is_none() && !outcome.cancelled {
        if state.budget_stop.as_ref().is_some_and(|stop| stop.limit == "review") {
            "reviewRejected"
        } else {
            "budgetReached"
        }
    } else {
        outcome.status()
    };
    // The last stage that actually said something. A Review that asked for
    // changes is the answer to the task, not the Verify that never ran.
    let summary = state
        .notes
        .iter()
        .rev()
        .map(|n| n.summary.clone())
        .find(|s| !s.trim().is_empty())
        .unwrap_or_else(|| outcome.summary());
    // Read before this task closes, so it is never its own comparison. A
    // baseline that cannot be read is no baseline, not a failed run.
    let baseline = serde_json::to_value(route.kind)
        .ok()
        .and_then(|kind| store.baseline(task_id, kind.as_str()?, id.program()).ok().flatten());
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
    }
}

/// Whether the next stage may start.
///
/// Three ceilings, checked in the order that a user would want to hear about
/// them. None of them can stop a turn that is already running, and none of them
/// ever answers "so run a bigger route instead" - a stop is a stop, and going
/// further is the user's call.
fn gate(route: &Route, state: &State, index: usize, stages: &[Stage]) -> Option<BudgetStop> {
    let remaining: Vec<Stage> = stages[index..].to_vec();
    let budget = &route.budget;
    let stop = |limit: &'static str, allowed: u64, observed: u64, message: String| {
        Some(BudgetStop { limit, allowed, observed, remaining: remaining.clone(), message })
    };

    if state.calls_used >= budget.max_agent_calls {
        return stop(
            "calls",
            budget.max_agent_calls.into(),
            state.calls_used.into(),
            format!(
                "This route was given {} agent call{}, and they are used up.                  The work so far is kept; running the rest is up to you.",
                budget.max_agent_calls,
                if budget.max_agent_calls == 1 { "" } else { "s" }
            ),
        );
    }
    if let Some(stop) = token_stop(budget.max_reported_tokens, state.reported_tokens(), remaining.clone()) {
        return Some(stop);
    }
    None
}

/// The stop a token ceiling leaves behind, once cumulative reported tokens are
/// past it. Usage only arrives with a completed turn, so this can only ever
/// act between turns - it never claims to interrupt one.
fn token_stop(limit: Option<u64>, used: u64, remaining: Vec<Stage>) -> Option<BudgetStop> {
    let limit = limit?;
    (used > limit).then(|| BudgetStop {
        limit: "tokens",
        allowed: limit,
        observed: used,
        remaining,
        message: format!(
            "This run has reported {used} tokens, past the {limit} this route budgeted. \
             Nothing further was started; the work so far is kept."
        ),
    })
}

/// Close out one stage: validate whatever artifact came back, log it, and hand
/// the next stage what it is entitled to.
fn finish_stage(store: &Store, ctx: &Context, state: &mut State, stage: Stage) -> StageNote {
    // An artifact counts only if it arrived as structured output and matches
    // the shape the stage contracted for. The alternative - reading the closing
    // prose and filling the fields from it - is exactly the guesswork the
    // schema exists to remove, so a missing artifact stays missing.
    let artifact = state
        .structured
        .take()
        .filter(|value| routing::artifact_is_valid(stage, value));
    if stage.schema().is_some() {
        let _ = note(store, ctx, "artifact", &artifact_payload(stage, artifact.as_ref()));
    }
    // A review that asks for changes ends the route here. Verifying a change
    // the review just rejected spends a call to confirm something already
    // known, and adding a fix call would be Orteca deciding to spend more on
    // the user's behalf. The findings are the result; what to do about them is
    // the user's to choose.
    // A review that hit a budget stop did not complete; that stop is the result.
    if stage == Stage::Review
        && state.budget_stop.is_none()
        && !artifact.as_ref().is_some_and(routing::review_passed)
    {
        let remaining = ctx.remaining.iter().skip(1).copied().collect();
        let stop = BudgetStop {
            limit: "review",
            allowed: 0,
            observed: 0,
            remaining,
            message: "The review did not return a valid pass. Its findings are kept; deciding whether to change the work is up to you.".into(),
        };
        state.halt = true;
        state.budget_stop = Some(stop.clone());
        let _ = note(store, ctx, "budget", &budget_payload("stopped", &stop));
    }
    StageNote { stage, summary: state.outcome.summary(), artifact }
}

/// The route, whole, as the event log stores it.
fn routing_payload(route: &Route) -> String {
    serde_json::json!({ "kind": "routing", "data": route }).to_string()
}

fn stage_payload(stage: Stage, index: usize, route: &Route) -> String {
    serde_json::json!({
        "kind": "stage",
        "data": {
            "stage": stage,
            "index": index,
            "of": route.stages.len(),
            "writes": stage.writes(),
            "schema": stage.schema().is_some(),
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
    let borrowed: Vec<&str> = launch.argv.iter().map(String::as_str).collect();
    // Counted before the spawn can fail: a process Orteca tried to start is a
    // call it spent, and hiding the failures would flatter the metric.
    state.calls_used = state.calls_used.saturating_add(1);
    state.turns_this_call = 0;
    let mut run = match proc::spawn(&ctx.program.to_string_lossy(), &borrowed, &ctx.dir) {
        Ok(run) => run,
        Err(e) => {
            state.outcome.failure = Some(format!("could not start {}: {e}", ctx.id.program()));
            return Next::Ended;
        }
    };
    // Prompt bytes bypass cmd.exe parsing and its command-line limit.
    if let Err(e) = run.send_line(&launch.opening).await {
        state.outcome.failure = Some(format!("could not send prompt to {}: {e}", ctx.id.program()));
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
                let events = ctx.id.parse_line(&value);
                // Each parser understands a subset of its CLI's event types and
                // silently drops the rest. Harmless for the stream, fatal for
                // the record: a Codex run whose tool calls all failed said so in
                // an item type this parser does not know, and the log kept no
                // trace of why the run did nothing. Keep the raw line. It is not
                // emitted - the UI has no shape for it - so the log stays
                // complete while the stream stays readable.
                if events.is_empty() {
                    state.unknown_events = state.unknown_events.saturating_add(1);
                    if let Err(e) = store.append_event(ctx.task_id, ctx.stage(), "unknown", ctx.id.program(), &value.to_string()) {
                        state.outcome.failure = Some(format!("could not record run event: {}", e.message));
                        break;
                    }
                }
                for event in events {
                    if let Err(e) = record(store, ctx.task_id, ctx.stage(), ctx.id, &event).and_then(|()| emit(&event)) {
                        state.outcome.failure = Some(format!("could not record or deliver run event: {}", e.message));
                        break;
                    }
                    if let ProviderEvent::Started { session_id } = &event {
                        state.session = Some(session_id.clone());
                    }
                    let provider_budget_reached = matches!(
                        &event,
                        ProviderEvent::Failed { kind: FailureKind::BudgetReached, .. }
                    );
                    if let ProviderEvent::Done { structured, turns, .. } = &event {
                        // Kept raw. Whether it is an artifact is decided by the
                        // stage's own contract once the stage is over.
                        if structured.is_some() {
                            state.structured = structured.clone();
                        }
                        state.turns_used = state.turns_used.saturating_add(*turns);
                        state.turns_this_call = state.turns_this_call.saturating_add(*turns);
                    }
                    state.outcome.absorb(&event);
                    // `absorb` quite correctly records provider failures, but
                    // this particular failure is the route's own ceiling. It
                    // must remain a budget outcome, and the process must be
                    // stopped before it can enter another stage.
                    if provider_budget_reached {
                        state.outcome.failure = None;
                        if state.budget_stop.is_none() {
                            let stop = turn_stop(ctx, state);
                            let _ = note(store, ctx, "budget", &budget_payload("stopped", &stop));
                            state.budget_stop = Some(stop);
                        }
                        state.outcome.finished = true;
                        run.cancel();
                        ending = Some(Next::Ended);
                    }
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
                if state.outcome.failure.is_some() { break; }
                // Codex has no turn flag at 0.154.0, so Orteca counts for it.
                // Between turns, never inside one: what this stops is the next
                // turn, and it says so rather than pretending to interrupt.
                // Not conditioned on the stage looking finished: Codex ends
                // every turn with what its parser calls a result, so waiting for
                // an "unfinished" process would mean never enforcing this at
                // all. A run that used every turn it was given did reach its
                // ceiling, and saying so is the honest reading.
                // The token ceiling is held to the same place: cumulative usage
                // across every stage so far, checked as each result reports it
                // (Claude reports only per result, which may span several turns),
                // so a stage that keeps taking turns stops at the first one
                // that crosses it rather than only at the next stage boundary.
                // A process already ending toward a resume is still drained
                // through here: a turn it completed may cross the ceiling, and
                // then the resume must not start.
                if !matches!(ending, Some(Next::Ended)) && state.budget_stop.is_none() {
                    // Claude enforces its own `--max-turns` and says so with
                    // `error_max_turns`; a result that used exactly the ceiling
                    // and succeeded did not run out.
                    let stop = if ctx.id != ProviderId::Claude
                        && ctx.plan.max_turns.is_some_and(|max| state.turns_this_call >= max)
                    {
                        Some(turn_stop(ctx, state))
                    } else {
                        token_stop(
                            ctx.max_reported_tokens,
                            state.reported_tokens(),
                            ctx.remaining.iter().skip(1).copied().collect(),
                        )
                    };
                    if let Some(stop) = stop {
                        let _ = note(store, ctx, "budget", &budget_payload("stopped", &stop));
                        state.budget_stop = Some(stop);
                        run.cancel();
                        ending = Some(Next::Ended);
                    }
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
        Control::Instruct { text, apply_now, reply } => {
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
                        Ok(()) => {
                            InstructionDisposition::Live
                        }
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
                state.outcome.failure = Some(format!("could not record the instruction: {}", e.message));
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
/// This resume is exempt from the call ceiling, deliberately. It exists to
/// deliver something the *user* asked for while the run was going, and the rule
/// the budget enforces is that Orteca never spends more on its own initiative.
/// Refusing here would lose an instruction the user was promised would arrive.
/// The extra call is still counted, and still logged.
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

/// The stop a turn ceiling leaves behind. The route's remaining stages are
/// listed because they are what the user is being asked to decide about.
fn turn_stop(ctx: &Context, state: &State) -> BudgetStop {
    let allowed = ctx.plan.max_turns.unwrap_or(0);
    BudgetStop {
        limit: "turns",
        allowed: allowed.into(),
        observed: state.turns_this_call.into(),
        remaining: ctx.remaining.iter().skip(1).copied().collect(),
        message: format!(
            "The {} stage reached its ceiling of {allowed} turns. What it had already \
             done is kept; carrying on is up to you.",
            ctx.stage()
        ),
    }
}

fn instruction(text: &str, disposition: InstructionDisposition) -> String {
    serde_json::json!({ "kind": "instruction", "data": { "text": text, "applied": disposition } }).to_string()
}

/// Log the event, then show it. The log is the record; the emit is the view.
fn record(store: &Store, task_id: i64, stage: &str, id: ProviderId, event: &ProviderEvent) -> crate::error::Result<()> {
    let payload = serde_json::to_string(event).map_err(|e| crate::error::AppError::new(crate::error::ErrorKind::Invalid, e.to_string()))?;
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
        StagePlan { stage, max_turns: Some(6), schema: write_schema(0, stage) }
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
        let dir = std::env::temp_dir().join(format!("orteca-stream-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&dir).status().unwrap().success());
        let project = store.touch_project(dir.to_str().unwrap(), "test").unwrap();
        Request {
            task_id: store.create_task(NewTask {
                project_id: project.id, prompt: "test", mode: "balanced", route_json: None,
                branch: None, base_commit: None, dirty_at_start: false,
            }).unwrap(),
            id: ProviderId::Codex, program: dir.join("fake.cmd"), dir,
            prompt: "a\"b %PATH% & ^\n\\ --help".into(), route: one_call("fix the typo"),
            base_commit: None, dirty_at_start: false, before_run: Some(Default::default()), recordings: None,
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
        let result = stream(&store, &Live::default(), request, |e| { events.borrow_mut().push(e.clone()); Ok(()) }).await;
        // The shim echoes whatever reached its stdin. What matters is that
        // the user's exact bytes survived the brief that wraps them, quotes,
        // percent signs, carets, backslashes and newlines included.
        assert!(result.summary.contains(&prompt), "prompt bytes were mangled: {}", result.summary);
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
        stream(&store, &Live::default(), request, |e| { emitted.borrow_mut().push(e.kind()); Ok(()) }).await;

        assert!(store.event_kinds(task).contains(&"unknown".to_string()), "unparsed line was dropped");
        let raw = store.event_payloads(task).into_iter()
            .find(|v| v["item"]["type"] == "unified_exec")
            .expect("raw payload was not kept verbatim");
        assert_eq!(raw["item"]["error"], "sandbox setup failed", "the reason the run did nothing must survive");
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
        let program = std::env::temp_dir().join(format!("orteca-read-only-{}.cmd", std::process::id()));
        let fixture = mock::named("codex-read-only-run.jsonl");
        std::fs::write(&program, format!("@echo off\r\ntype \"{}\"\r\n", fixture.display())).unwrap();
        request.program = program.clone();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert!(result.diff.is_empty());
        assert_eq!(result.status, "failed");
        assert!(result.failure.unwrap().contains("without changing any file"));
        // The agent's own words stay the summary; they were never the signal.
        assert!(result.summary.contains("read-only"));
        std::fs::remove_file(program).unwrap();
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
        assert_eq!(written.lines().count(), 2, "non-JSON output leaked in: {written}");

        let replayed: Vec<String> = crate::providers::mock::replay(ProviderId::Codex, &recording)
            .expect("a recording must load as a fixture")
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        assert_eq!(replayed, live.into_inner(), "replay must reproduce the run it recorded");

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
        assert_eq!(result.failure, None, "a stop the user asked for is not a failure");
        assert_eq!(result.summary, "half an answer", "what the agent already said is kept");
        assert!(store.event_kinds(task).contains(&"cancel".to_string()), "the log must record the stop");
        // The sender goes with the run, so a late second click cannot claim to
        // have stopped something that is already over.
        assert!(live.send(task, Control::Cancel).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_control_cannot_reach_a_run_that_is_not_live() {
        let live = Live::default();
        assert!(live.send(7, Control::Cancel).is_err(), "nothing to stop yet");
        let listening = live.open(7);
        assert!(live.send(7, Control::Cancel).is_ok());
        live.close(7);
        assert!(live.send(7, Control::Cancel).is_err(), "a finished run cannot be stopped");
        drop(listening);
    }

    /// A result is not the same thing as an ending. A live provider reports one
    /// per turn, so a stop during turn two must still read as a stop even
    /// though turn one already answered - only `finished` means the run is over.
    #[test]
    fn a_stop_is_rewritten_by_the_run_ending_but_not_by_a_single_turn() {
        let mut answered = Outcome::default();
        answered.absorb(&ProviderEvent::Done { result: "shipped".into(), structured: None, turns: 1 });
        answered.finished = true;
        answered.cancelled = true;
        assert_eq!(answered.status(), "done", "the run had already ended");

        let mut mid_turn = Outcome::default();
        mid_turn.absorb(&ProviderEvent::Done { result: "turn one".into(), structured: None, turns: 1 });
        mid_turn.cancelled = true;
        assert!(mid_turn.done, "a turn did answer");
        assert_eq!(mid_turn.status(), "cancelled", "but the run was stopped mid-work");

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
                let receipt = live.instruct(task, "also tidy up".into(), false).await.unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Live);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert!(result.summary.ends_with("also tidy up"), "the live instruction was not incorporated");
        let usage = result.usage.expect("a steered run still reports usage");
        assert_eq!(usage.output_tokens, 1, "one result must be counted once");
        assert_eq!(usage.cost_usd, Some(0.01));

        let payload = store.event_payloads(task).into_iter()
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
        std::fs::write(dir.join("fake.js"), concat!(
            "process.stdin.resume();",
            "console.log(JSON.stringify({type:'result',subtype:'success',result:'done',",
            "usage:{input_tokens:1,output_tokens:1},total_cost_usd:0.01}));",
            "process.stdin.on('end',()=>setTimeout(()=>process.exit(0),500));",
        )).unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, receipt) = tokio::join!(
            stream(&store, &live, request, move |e| { let _ = heard.send(e.kind()); Ok(()) }),
            async {
                wait_for_kind(&mut hearing, "done").await;
                live.instruct(task, "one more thing".into(), false).await.unwrap()
            }
        );

        assert_eq!(receipt.disposition, InstructionDisposition::TooLate);
        assert_eq!(result.status, "done", "{:?}", result.failure);
        let payload = store.event_payloads(task).into_iter()
            .find(|v| v["kind"] == "instruction").expect("the late instruction was not logged");
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
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n").unwrap();
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
            stream(&store, &live, request, move |e| { let _ = heard.send(e.kind()); Ok(()) }),
            async {
                wait_for_kind(&mut hearing, "text").await;
                let receipt = live.instruct(task, "use tabs".into(), false).await.unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Held);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert_eq!(result.summary, "boundary use tabs");
        let payload = store.event_payloads(task).into_iter()
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
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n").unwrap();
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
            stream(&store, &live, request, move |e| { let _ = heard.send(e.kind()); Ok(()) }),
            async {
                wait_for_kind(&mut hearing, "text").await;
                let receipt = live.instruct(task, "make it faster".into(), true).await.unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Resumed);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert_eq!(result.summary, "resumed sess-1 with make it faster");
        let payload = store.event_payloads(task).into_iter()
            .find(|v| v["kind"] == "instruction")
            .expect("the instruction was not logged");
        assert_eq!(payload["data"]["applied"], "resumed");
        // One task across two processes: the first one's words are still here.
        let texts: Vec<String> = store.event_payloads(task).into_iter()
            .filter(|v| v["kind"] == "text")
            .map(|v| v["data"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(texts.contains(&"working".to_string()), "the log lost the first process");
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
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\" %*\r\n").unwrap();
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
            stream(&store, &live, request, move |e| { let _ = heard.send(e.kind()); Ok(()) }),
            async {
                wait_for_kind(&mut hearing, "text").await;
                let receipt = live.instruct(task, "hurry".into(), true).await.unwrap();
                assert_eq!(receipt.disposition, InstructionDisposition::Held);
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert_eq!(result.summary, "late hurry");
        let payload = store.event_payloads(task).into_iter()
            .find(|v| v["kind"] == "instruction").expect("not logged");
        assert_eq!(payload["data"]["applied"], "held", "the receipt was honest while the session was pending");
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
        assert!(store.event_payloads(task).iter().any(|v| v["kind"] == "failed"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn event_storage_and_delivery_failures_cannot_report_success() {
        for reject_storage in [false, true] {
            let store = Store::in_memory().unwrap();
            let request = task_request(&store, if reject_storage { "storage-failure" } else { "delivery-failure" });
            let dir = request.dir.clone();
            std::fs::write(&request.program, "@echo off\r\necho {\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":1}}\r\nexit /b 0\r\n").unwrap();
            if reject_storage { store.reject_events(); }
            let result = stream(&store, &Live::default(), request, |_| {
                if reject_storage { Ok(()) } else { Err(crate::error::AppError::new(crate::error::ErrorKind::Io, "window unavailable")) }
            }).await;
            assert_eq!(result.status, "failed");
            assert!(result.failure.unwrap().contains(if reject_storage { "disk unavailable" } else { "window unavailable" }));
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
                assert!(!args(id, &plan_for(Stage::Implement)).contains(&prompt.to_string()), "{id:?}: {prompt:?}");
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
        // A headless run has nobody to answer a prompt, so it must never wait
        // for one. This is what made an agent stall instead of running a test.
        assert!(claude.contains("--permission-prompts none"));
        // The agent has to be able to verify its own work.
        assert!(claude.contains("--allowedTools Bash PowerShell"));

        // Deny beats allow, and every rule covers both shells.
        for command in CLAUDE_DENY_COMMANDS {
            assert!(claude.contains(&format!("Bash({command})")), "{command}");
            assert!(claude.contains(&format!("PowerShell({command})")), "{command}");
        }
        assert!(claude.contains("Bash(git push:*)"));
        assert!(claude.contains("Bash(rm:*)"));
        assert!(claude.contains("PowerShell(Remove-Item:*)"));
        // Granting the shells must not silently outrank the denylist.
        let allow = claude.find("--allowedTools").expect("allow");
        let deny = claude.find("--disallowedTools").expect("deny");
        assert!(allow < deny, "denylist must come after the grant it narrows");

        let codex = args(ProviderId::Codex, &plan_for(Stage::Implement)).join(" ");
        assert!(codex.contains("--sandbox workspace-write"));
        assert!(!codex.contains("danger-full-access"));
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
                assert!(claude.iter().any(|a| a == tool), "{} could still call {tool}", stage.name());
            }
            assert!(claude.contains(&"Read".to_string()), "{} lost read access", stage.name());
            assert!(!claude.contains(&"Bash".to_string()), "{} received arbitrary Bash", stage.name());
            assert!(!claude.contains(&"PowerShell".to_string()), "{} received arbitrary PowerShell", stage.name());
        }
        // Implement still writes, or nothing would ever change.
        let codex = args(ProviderId::Codex, &plan_for(Stage::Implement)).join(" ");
        assert!(codex.contains("--sandbox workspace-write"));
        let claude = args(ProviderId::Claude, &plan_for(Stage::Implement));
        for tool in CLAUDE_EDIT_TOOLS {
            assert!(!claude.iter().any(|a| a == tool), "Implement lost {tool}");
        }
    }

    /// Both budget flags are the ones the installed CLIs actually accept,
    /// checked against them rather than taken from the spec. Claude has a turn
    /// ceiling and takes its schema inline; Codex has neither a turn flag nor
    /// an inline schema, and takes a file.
    #[test]
    fn only_verified_ceiling_flags_reach_a_command_line() {
        let plan = StagePlan { stage: Stage::Plan, max_turns: Some(7), schema: write_schema(0, Stage::Plan) };
        let claude = args(ProviderId::Claude, &plan);
        assert!(claude.windows(2).any(|w| w == ["--max-turns", "7"]));
        assert!(claude.contains(&"--json-schema".to_string()));
        assert!(claude.contains(&routing::PLAN_SCHEMA.to_string()));

        let codex = args(ProviderId::Codex, &plan);
        assert!(!codex.contains(&"--max-turns".to_string()), "codex 0.154.0 has no turn flag");
        assert!(!codex.contains(&"--json-schema".to_string()), "codex takes a file, not inline JSON");
        let at = codex.iter().position(|a| a == "--output-schema").expect("codex takes a schema file");
        let path = std::path::Path::new(&codex[at + 1]);
        assert_eq!(std::fs::read_to_string(path).unwrap(), routing::PLAN_SCHEMA);
        // Never in the user's repository: a schema file in their diff would be
        // Orteca editing their project.
        assert!(!path.starts_with(std::env::current_dir().unwrap()));

        // A stage with no artifact contract asks for none.
        let implement = args(ProviderId::Claude, &plan_for(Stage::Implement));
        assert!(!implement.contains(&"--json-schema".to_string()));
    }

    /// A shim that answers every call, logs the brief it was given, and exits.
    /// `turns` is how many turns it reports before it stops, which is what a
    /// turn ceiling is measured against.
    fn answering_shim(request: &Request, turns: usize) {
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        let log = request.dir.join("briefs.log").to_string_lossy().replace('\\', "/");
        std::fs::write(
            request.dir.join("fake.js"),
            format!(
                "const fs=require('fs');let input='';process.stdin.setEncoding('utf8');\
                 process.stdin.on('data',s=>input+=s);process.stdin.on('end',()=>{{\
                 fs.appendFileSync('{log}','=== CALL ===\\n'+input);\
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
        request.id = ProviderId::Claude;
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        let log = request.dir.join("briefs.log").to_string_lossy().replace('\\', "/");
        std::fs::write(
            request.dir.join("fake.js"),
            format!(
                "const fs=require('fs');let buf='',answered=false;process.stdin.setEncoding('utf8');\
                 process.stdin.on('data',d=>{{buf+=d;const ls=buf.split('\\n');buf=ls.pop();\
                 for(const l of ls){{if(!l.trim()||answered)continue;answered=true;\
                 fs.appendFileSync('{log}','=== CALL ===' + JSON.parse(l).message.content);\
                 console.log(JSON.stringify({{type:'result',subtype:'success',result:'stage answered',\
                 structured_output:{structured},usage:{{input_tokens:10,output_tokens:1}},total_cost_usd:0.01}}));}}}});\
                 process.stdin.on('end',()=>process.exit(0));"
            ),
        )
        .unwrap();
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
        assert_eq!(result.calls_used, 1, "a trivial task started more than one process");
        assert_eq!(result.route.budget.max_agent_calls, 1);
        assert_eq!(result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(), [Stage::Implement]);
        assert!(result.budget_stop.is_none());

        let calls = briefs(&dir);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains("one focused check"), "verification was not asked for in the one call");

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
    async fn the_route_and_its_ceilings_are_recorded_before_anything_runs() {
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
        assert_eq!(routing["data"]["budget"]["maxAgentCalls"], 1);
        assert!(routing["data"]["budget"]["maxTurns"].is_number());
        assert!(routing["data"]["signals"]["complexity"].is_number());
        assert!(routing["data"]["reason"].as_str().is_some_and(|r| !r.is_empty()));
        // The tier is recorded and, deliberately, reaches no command line.
        assert!(routing["data"]["budget"]["preferredTier"].is_string());
        // Before the first provider event, not after.
        let first_provider = payloads.iter().position(|v| v["kind"] == "started" || v["kind"] == "text");
        let at = payloads.iter().position(|v| v["kind"] == "routing").unwrap();
        assert!(first_provider.is_none_or(|p| at < p), "the route was recorded after the run began");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A longer route runs its stages in order, and each one is told what it is
    /// for. Only Implement is allowed to write.
    #[tokio::test]
    async fn a_longer_route_runs_its_stages_in_order() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "add an authorization check before the delete endpoint",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        assert_eq!(route.stages, [Stage::Plan, Stage::Implement, Stage::Review, Stage::Verify]);
        let mut request = routed(&store, "stages", route);
        let dir = request.dir.clone();
        claude_shim(
            &mut request,
            &serde_json::json!({
                "findings": [],
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
        assert!(calls[0].contains("Do not edit any file"), "the plan stage was allowed to edit");
        assert!(calls[1].contains("Make the change"));
        assert!(calls[3].contains("Verify"));
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

    /// Running out of calls is its own outcome. The diff, the usage and the
    /// work so far all survive it, and no stage starts afterwards.
    #[tokio::test]
    async fn a_route_that_runs_out_of_calls_stops_and_keeps_its_work() {
        let store = Store::in_memory().unwrap();
        let mut route = routing::route("redesign the storage subsystem", Mode::Balanced, &RepoSignals::default());
        assert_eq!(route.stages.len(), 3);
        // A ceiling below the route's own length: what a rolled-back budget, or
        // a resume the user asked for, would leave behind.
        route.budget.max_agent_calls = 2;
        let request = routed(&store, "out-of-calls", route);
        let (dir, task) = (request.dir.clone(), request.task_id);
        answering_shim(&request, 1);
        // Something in the working tree, so "the work is kept" is testable.
        std::fs::write(dir.join("touched.txt"), "work the agent did").unwrap();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "budgetReached");
        assert_eq!(result.failure, None, "a budget stop is not a failure");
        assert_eq!(result.calls_used, 2, "a third call was started anyway");
        assert_eq!(briefs(&dir).len(), 2);

        let stop = result.budget_stop.expect("no budget stop was reported");
        assert_eq!(stop.limit, "calls");
        assert_eq!(stop.allowed, 2);
        assert_eq!(stop.remaining, [Stage::Verify], "the user is not told what is left");
        assert!(stop.message.contains("up to you"), "the stop must hand the choice back");

        // Work, diff and usage all preserved.
        assert!(result.diff.iter().any(|f| f.path.contains("touched.txt")), "the diff was lost");
        assert!(result.usage.is_some(), "usage was lost");
        assert!(!result.summary.is_empty(), "what the agent said was lost");
        // And the route is unchanged: a stop never rewrites itself into a
        // bigger budget so it can carry on.
        assert_eq!(result.route.budget.max_agent_calls, 2);
        assert_eq!(result.route.stages.len(), 3);

        let budget: Vec<_> = store.event_payloads(task).into_iter().filter(|v| v["kind"] == "budget").collect();
        assert!(budget.iter().any(|v| v["data"]["decision"] == "stopped" && v["data"]["limit"] == "calls"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_turn_budget_stop_prevents_later_stages() {
        let store = Store::in_memory().unwrap();
        let mut route = routing::route("redesign the storage subsystem", Mode::Balanced, &RepoSignals::default());
        route.budget.max_turns = Some(1);
        let request = routed(&store, "turn-stop-route", route);
        let dir = request.dir.clone();
        answering_shim(&request, 3);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "budgetReached");
        assert_eq!(result.failure, None);
        assert_eq!(result.calls_used, 1, "the next stage started after a turn stop");
        assert_eq!(
            result.stages.iter().map(|n| n.stage).collect::<Vec<_>>(),
            [Stage::Plan]
        );
        let stop = result.budget_stop.expect("no turn budget stop");
        assert_eq!(stop.limit, "turns");
        assert_eq!(stop.remaining, [Stage::Implement, Stage::Verify]);
        assert_eq!(briefs(&dir).len(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The rule the milestone turns on: after a budget stop nothing escalates.
    /// Not a longer route, not one more call, not a quiet retry.
    #[tokio::test]
    async fn a_budget_stop_never_escalates_by_itself() {
        let store = Store::in_memory().unwrap();
        let mut route = routing::route("redesign the storage subsystem", Mode::Balanced, &RepoSignals::default());
        route.budget.max_agent_calls = 1;
        let before = route.clone();
        let request = routed(&store, "no-escalation", route);
        let (dir, task) = (request.dir.clone(), request.task_id);
        answering_shim(&request, 1);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "budgetReached");
        assert_eq!(result.calls_used, 1);
        assert_eq!(briefs(&dir).len(), 1, "another provider process was started after the stop");
        assert_eq!(result.route, before, "the route rewrote itself after being stopped");
        // Only the first stage ever ran.
        let stages: Vec<String> = store
            .event_payloads(task)
            .into_iter()
            .filter(|v| v["kind"] == "stage")
            .map(|v| v["data"]["stage"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(stages, ["plan"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_claude_result_that_used_every_turn_and_succeeded_is_done() {
        let store = Store::in_memory().unwrap();
        let mut route = one_call("fix the typo");
        route.budget.max_turns = Some(2);
        let mut request = routed(&store, "claude-full-turns", route);
        request.id = ProviderId::Claude;
        let dir = request.dir.clone();
        std::fs::write(&request.program, "@echo off
echo {\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":2,\"result\":\"ok\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}
exit /b 0
").unwrap();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "done");
        assert!(result.budget_stop.is_none());
        assert_eq!(result.turns_used, 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn claude_max_turns_is_reported_as_a_budget_stop() {
        let store = Store::in_memory().unwrap();
        let mut request = routed(&store, "claude-turn-stop", one_call("fix the typo"));
        request.id = ProviderId::Claude;
        let dir = request.dir.clone();
        std::fs::write(&request.program, "@echo off\r\necho {\"type\":\"result\",\"subtype\":\"error_max_turns\",\"is_error\":true,\"result\":\"ceiling\",\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}\r\nexit /b 0\r\n").unwrap();

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "budgetReached");
        assert_eq!(result.failure, None);
        assert_eq!(result.budget_stop.as_ref().map(|s| s.limit), Some("turns"));
        assert!(result.usage.is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Codex has no turn flag, so Orteca counts turns for it. The ceiling ends
    /// the stage between turns - it does not claim to interrupt one - and what
    /// the stage already did is kept.
    #[tokio::test]
    async fn a_turn_ceiling_ends_a_stage_without_failing_it() {
        let store = Store::in_memory().unwrap();
        let mut route = one_call("fix the typo in the readme");
        route.budget.max_turns = Some(2);
        let request = routed(&store, "turns", route);
        let dir = request.dir.clone();
        // Four turns offered against a ceiling of two.
        answering_shim(&request, 4);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "budgetReached");
        assert_eq!(result.failure, None);
        let stop = result.budget_stop.expect("no budget stop was reported");
        assert_eq!(stop.limit, "turns");
        assert_eq!(stop.allowed, 2);
        assert!(result.turns_used >= 2);
        assert!(!result.summary.is_empty(), "the work the stage did was thrown away");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A token ceiling is an inter-turn guard and says so. It cannot stop a
    /// turn that is already running, so what it stops is the next stage.
    #[tokio::test]
    async fn a_token_ceiling_stops_the_next_stage_not_the_running_one() {
        let store = Store::in_memory().unwrap();
        let mut route = routing::route("redesign the storage subsystem", Mode::Balanced, &RepoSignals::default());
        // Below what one turn of the shim reports, so the first stage completes
        // and the second never starts.
        route.budget.max_reported_tokens = Some(1);
        let request = routed(&store, "tokens", route);
        let dir = request.dir.clone();
        answering_shim(&request, 1);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "budgetReached");
        assert_eq!(result.calls_used, 1, "the stage that was already running was cut short");
        let stop = result.budget_stop.expect("no budget stop was reported");
        assert_eq!(stop.limit, "tokens");
        assert!(stop.observed > stop.allowed);
        assert_eq!(stop.remaining, [Stage::Implement, Stage::Verify]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Cumulative usage is checked as each turn reports it, not only between
    /// stages: a one-stage route has no boundary at all, and still stops at the
    /// turn that crossed its ceiling.
    #[tokio::test]
    async fn cumulative_usage_stops_a_stage_between_turns() {
        let store = Store::in_memory().unwrap();
        let mut route = one_call("fix the typo in the readme");
        // 11 tokens a turn, so the third turn crosses it.
        route.budget.max_reported_tokens = Some(25);
        let request = routed(&store, "token-turns", route);
        let dir = request.dir.clone();
        answering_shim(&request, 4);

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "budgetReached");
        assert_eq!(result.failure, None);
        let stop = result.budget_stop.expect("no token stop was reported");
        assert_eq!(stop.limit, "tokens");
        assert_eq!(stop.observed, 33, "the stop was not taken at the turn that crossed the ceiling");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Milestone 5's promise, kept across a route: an instruction given during
    /// one stage reaches every stage after it, and is not delivered twice by
    /// resuming the stage it arrived in.
    #[tokio::test]
    async fn an_instruction_given_in_one_stage_carries_into_the_next() {
        let store = Store::in_memory().unwrap();
        let route = routing::route("redesign the storage subsystem", Mode::Balanced, &RepoSignals::default());
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
                let _ = live.instruct(task, "never touch the public API".into(), false).await;
            }
        );

        assert_eq!(result.status, "done");
        // Three stages, three calls: the instruction did not buy an extra
        // process to say the same thing twice.
        assert_eq!(result.calls_used, 3);
        let calls = briefs(&dir);
        assert!(
            calls[1..].iter().all(|b| b.contains("never touch the public API")),
            "a later stage lost the instruction: {calls:?}"
        );
        assert!(
            calls[1..].iter().all(|b| b.contains("Standing instructions")),
            "the instruction was not carried as a standing constraint"
        );
        // And it is in the log whatever became of it.
        assert!(store.event_kinds(task).contains(&"instruction".to_string()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A review that asks for changes ends the route there. Orteca does not add
    /// a fix call the user did not ask for, and does not spend a Verify call
    /// confirming something the review has already rejected.
    #[tokio::test]
    async fn a_review_that_asks_for_changes_does_not_buy_another_call() {
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
        assert_eq!(result.calls_used, 3, "a call was spent after the review rejected the work");
        assert_eq!(result.stages.len(), 3, "Verify ran after a review that rejected the work");
        assert_eq!(result.stages.last().unwrap().stage, Stage::Review);
        let artifact = result.stages.last().unwrap().artifact.as_ref().expect("the review artifact was dropped");
        assert_eq!(artifact["verdict"], "changes_requested");
        let stop = result.budget_stop.as_ref().expect("review stop was not returned");
        assert_eq!(stop.limit, "review");
        assert_eq!(stop.remaining, [Stage::Verify]);
        // Recorded, so the findings are not only on screen.
        let logged = store.event_payloads(task).into_iter().find(|v| v["kind"] == "artifact" && v["data"]["stage"] == "review");
        assert_eq!(logged.expect("no artifact row")["data"]["valid"], true);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_missing_review_artifact_rejects_the_route_without_verify() {
        let store = Store::in_memory().unwrap();
        let route = routing::route(
            "add an authorization check before the delete endpoint",
            Mode::Balanced,
            &RepoSignals::default(),
        );
        let mut request = routed(&store, "missing-review", route);
        let dir = request.dir.clone();
        claude_shim(&mut request, &serde_json::json!({"objective": "not a review"}));

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        assert_eq!(result.status, "reviewRejected");
        assert_eq!(result.stages.len(), 3);
        assert_eq!(result.stages.last().unwrap().stage, Stage::Review);
        assert!(result.stages.last().unwrap().artifact.is_none());
        assert_eq!(result.budget_stop.as_ref().map(|s| s.limit), Some("review"));
        std::fs::remove_dir_all(dir).unwrap();
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
        claude_shim(&mut request, &serde_json::json!({"objective": "half a plan"}));

        let result = stream(&store, &Live::default(), request, |_| Ok(())).await;

        let note = result.stages.first().expect("the stage left no note");
        assert!(note.artifact.is_none(), "a half-built plan was accepted as a plan");
        // The words are kept as words, to be forwarded verbatim and labelled
        // unvalidated. They are never mined for the fields the schema would
        // have filled.
        assert_eq!(note.summary, "stage answered");
        let logged = store.event_payloads(task).into_iter().find(|v| v["kind"] == "artifact").expect("no artifact row");
        assert_eq!(logged["data"]["valid"], false, "a missing artifact must be recorded as missing");
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
