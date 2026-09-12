//! A single-stage run: prompt -> one provider CLI -> stream -> diff -> result.
//!
//! Routing and extra stages are later milestones, and mid-task instructions are
//! the other half of this one. What is here is the whole honest path for a
//! single task: build the argv, spawn inside the Job Object, normalise every
//! line through the provider's parser, write each event to the append-only log,
//! let the user stop it, and end with a diff they can check.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tokio::sync::mpsc;

use crate::error::{AppError, ErrorKind};
use crate::proc::{self, Line};
use crate::project::{self, FileStat};
use crate::providers::{claude, ProviderEvent, ProviderId, Steering, Usage};
use crate::store::Store;

/// One stage, so one name. Routing gives these real names in Milestone 6.
const STAGE: &str = "run";

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

/// Every denied command in both shells Claude can reach, so a blocked command
/// cannot simply be rerun through the other one.
fn claude_deny() -> Vec<String> {
    CLAUDE_DENY_COMMANDS
        .iter()
        .flat_map(|cmd| [format!("Bash({cmd})"), format!("PowerShell({cmd})")])
        .collect()
}

/// What a user can still do to a run that is already going.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Stop now. Whatever the agent already wrote to the working tree stays
    /// written: Orteca captures a diff, it never reverts the user's files.
    Cancel,
    /// Words for the agent while it works. Where they go depends on the
    /// provider: a live one takes them mid-turn, a checkpoint one holds them.
    /// `apply_now` is the user choosing not to wait for a boundary that a
    /// single-stage run never reaches - it ends the process and resumes the
    /// session carrying the instruction.
    Instruct { text: String, apply_now: bool },
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
}

/// The `task_events` row a stop leaves behind. Not a `ProviderEvent`: the
/// provider did not say this, the user did. Without it a run stopped two
/// seconds in is indistinguishable afterwards from one that died on its own.
const CANCEL_PAYLOAD: &str = r#"{"kind":"cancel","data":{"by":"user"}}"#;

/// What the UI gets when the run ends.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub task_id: i64,
    /// `done`, `cancelled` or `failed`.
    pub status: &'static str,
    /// The provider's final answer, or its last message if it reports no final
    /// field. Empty is possible and is not an error.
    pub summary: String,
    pub failure: Option<String>,
    /// `None` when the run ended before the provider reported any numbers.
    /// The UI must say "unavailable" and never print a zero.
    pub usage: Option<Usage>,
    pub diff: Vec<FileStat>,
    /// The diff includes edits that were already in the working tree.
    pub dirty_at_start: bool,
}

/// The argv for one run.
///
/// Claude is never given `--bare`: that would disable OAuth and force an API
/// key, billing the user instead of using the subscription they already have.
/// Codex is never given `--sandbox danger-full-access`; `workspace-write` is
/// the widest access Orteca asks for.
pub fn args(id: ProviderId, _prompt: &str) -> Vec<String> {
    let arg = str::to_string;
    match id {
        ProviderId::Codex => vec![
            arg("exec"),
            arg("-"),
            arg("--json"),
            arg("--sandbox"),
            arg("workspace-write"),
        ],
        ProviderId::Claude => [vec![
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
            // acceptEdits only auto-approves edits. Without this the agent can
            // write code but never run the test that proves the code works.
            arg("--allowedTools"),
            arg("Bash"),
            arg("PowerShell"),
            arg("--disallowedTools"),
        ], claude_deny()].concat(),
    }
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
    pub id: ProviderId,
    pub program: PathBuf,
    pub dir: PathBuf,
    pub prompt: String,
    pub base_commit: Option<String>,
    pub dirty_at_start: bool,
    /// Where to keep this run's raw JSONL. `None` records nothing.
    pub recording: Option<PathBuf>,
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
    fn new(path: Option<PathBuf>) -> Self {
        Self { path, file: None }
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
    fn first(id: ProviderId, prompt: &str) -> Self {
        Launch {
            argv: args(id, prompt),
            opening: match id {
                ProviderId::Claude => claude::user_message(prompt),
                ProviderId::Codex => prompt.to_string(),
            },
        }
    }

    /// Pick a recorded session back up with everything the user has said since.
    /// The session already holds the history, so only the new words are sent.
    fn resume(id: ProviderId, session: &str, held: &[String]) -> Self {
        Launch {
            argv: id.resume_args(session),
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
}

/// Everything one task carries across a restart. A resumed Codex session is
/// still the same task: the same event log, the same token total, the same
/// diff baseline, the same recording.
struct State {
    outcome: Outcome,
    recording: Recording,
    /// Non-JSON output - the only clue a CLI leaves when it dies badly.
    noise: Vec<String>,
    /// Instructions the provider has not taken yet. A checkpoint provider
    /// accumulates these; a live one never holds anything.
    held: Vec<String>,
    /// The provider's own session id, which is what a resume needs.
    session: Option<String>,
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
    let Request { task_id, id, program, dir, prompt, base_commit, dirty_at_start, recording } = request;
    // Registered before the CLI is even spawned: a run is stoppable from the
    // moment the user can see it, including while a slow Node shim starts up.
    let mut control = live.open(task_id);
    let ctx = Context { task_id, id, program, dir };
    let mut state = State {
        outcome: Outcome::default(),
        recording: Recording::new(recording),
        noise: Vec::new(),
        held: Vec::new(),
        session: None,
    };

    // Usually one pass. A checkpoint provider told to apply an instruction now
    // ends its process and comes back through here resuming its own session.
    let mut launch = Launch::first(id, &prompt);
    loop {
        match attempt(store, &ctx, &mut state, &mut control, &emit, launch).await {
            Next::Ended => break,
            Next::Restart(again) => launch = again,
        }
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
        Ok(diff) => diff,
        Err(e) => {
            outcome.failure.get_or_insert(e.message);
            Vec::new()
        }
    };
    if let Some(message) = &outcome.failure {
        let event = ProviderEvent::Failed { kind: crate::providers::classify_failure(message), message: message.clone() };
        if let Err(e) = record(store, task_id, id, &event) {
            outcome.failure = Some(format!("{message}; could not log failure: {}", e.message));
        }
    }
    let mut status = outcome.status();
    let summary = outcome.summary();
    if let Err(e) = store.finish_task(
        task_id,
        status,
        &summary,
        &serde_json::to_string(&diff).unwrap_or_else(|_| "[]".into()),
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
            dirty_at_start,
    }
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
    let State { outcome, recording, noise, held, session } = state;

    let borrowed: Vec<&str> = launch.argv.iter().map(String::as_str).collect();
    let mut run = match proc::spawn(&ctx.program.to_string_lossy(), &borrowed, &ctx.dir) {
        Ok(run) => run,
        Err(e) => {
            outcome.failure = Some(format!("could not start {}: {e}", ctx.id.program()));
            return Next::Ended;
        }
    };
    // Prompt bytes bypass cmd.exe parsing and its command-line limit.
    if let Err(e) = run.send_line(&launch.opening).await {
        outcome.failure = Some(format!("could not send prompt to {}: {e}", ctx.id.program()));
    }
    // A checkpoint provider is told no more input is coming. A live one keeps
    // its stdin, because that is what an instruction travels down - which also
    // means the run ends when Orteca closes stdin rather than when the provider
    // reports a result. `claude --input-format stream-json` sits waiting for
    // another turn indefinitely.
    if ctx.id.steering() == Steering::Checkpoint {
        run.close_stdin();
    }

    // Turns Orteca is still waiting on. A live provider answers one user
    // message per turn, and stdin has to stay open until the last one has been
    // answered: closing it at the first result would cut off an instruction
    // that was sent while that turn was still running.
    let mut owed: u32 = 1;
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
                ending = answer(action, store, ctx, outcome, held, session.as_deref(), &mut run, &mut owed).await;
                continue;
            }
        };
        let Some(line) = next else { break };
        match line {
            Line::Json(value) => {
                // Before parsing: the recording is what the CLI said, not what
                // Orteca understood of it.
                recording.write(&value);
                let events = ctx.id.parse_line(&value);
                // Each parser understands a subset of its CLI's event types and
                // silently drops the rest. Harmless for the stream, fatal for
                // the record: a Codex run whose tool calls all failed said so in
                // an item type this parser does not know, and the log kept no
                // trace of why the run did nothing. Keep the raw line. It is not
                // emitted - the UI has no shape for it - so the log stays
                // complete while the stream stays readable.
                if events.is_empty() {
                    if let Err(e) = store.append_event(ctx.task_id, STAGE, "unknown", ctx.id.program(), &value.to_string()) {
                        outcome.failure = Some(format!("could not record run event: {}", e.message));
                        break;
                    }
                }
                for event in events {
                    if let Err(e) = record(store, ctx.task_id, ctx.id, &event).and_then(|()| emit(&event)) {
                        outcome.failure = Some(format!("could not record or deliver run event: {}", e.message));
                        break;
                    }
                    if let ProviderEvent::Started { session_id } = &event {
                        *session = Some(session_id.clone());
                    }
                    outcome.absorb(&event);
                    if matches!(event, ProviderEvent::Done { .. }) {
                        match ctx.id.steering() {
                            // The CLI ends itself once its one turn is done.
                            Steering::Checkpoint => outcome.finished = true,
                            Steering::Live => {
                                // A result per turn, not per run. The run ends
                                // when every message has been answered and
                                // Orteca closes stdin - never before, or an
                                // instruction sent during this turn is lost.
                                owed = owed.saturating_sub(1);
                                if owed == 0 {
                                    outcome.finished = true;
                                    run.close_stdin();
                                }
                            }
                        }
                    }
                }
                if outcome.failure.is_some() { break; }
            }
            Line::Text(text) => {
                if !text.trim().is_empty() {
                    noise.push(text);
                    // Only the tail matters for a failure message.
                    if noise.len() > 5 {
                        noise.remove(0);
                    }
                }
            }
            Line::Exit(code) => {
                // A process ended on purpose - stopped by the user, or handed
                // over to a resume - has nothing to explain.
                if ending.is_none() {
                    outcome.exited(ctx.id, code, noise);
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
    outcome: &mut Outcome,
    held: &mut Vec<String>,
    session: Option<&str>,
    run: &mut proc::Run,
    owed: &mut u32,
) -> Option<Next> {
    match action {
        Control::Cancel => {
            // Logged before the kill, because after it there may be no run
            // left to log anything.
            if let Err(e) = note(store, ctx, "cancel", CANCEL_PAYLOAD) {
                outcome.failure = Some(format!("could not record the stop: {}", e.message));
                return Some(Next::Ended);
            }
            outcome.cancelled = true;
            // Not a drop: the exit waiter still has the last of the CLI's
            // output to hand over.
            run.cancel();
            Some(Next::Ended)
        }
        Control::Instruct { text, apply_now } => {
            // A resume needs a session to resume, and a provider only reports
            // one once it has started talking. Without it the instruction waits
            // rather than appearing to have been applied.
            let resume = match (ctx.id.steering(), apply_now, session) {
                (Steering::Checkpoint, true, Some(session)) => Some(session.to_string()),
                _ => None,
            };
            let applied = match ctx.id.steering() {
                Steering::Live => {
                    // Straight down stdin, mid-turn. The only way this fails is
                    // a run whose stdin Orteca has already closed, which means
                    // the instruction arrived after the last turn ended.
                    match run.send_line(&claude::user_message(&text)).await {
                        Ok(()) => {
                            // One more turn to wait for before stdin may close.
                            *owed += 1;
                            "live"
                        }
                        Err(_) => "tooLate",
                    }
                }
                Steering::Checkpoint => {
                    held.push(text.clone());
                    if resume.is_some() { "resumed" } else { "held" }
                }
            };
            // Never lost, whatever became of it: the task log is the record of
            // what the user asked for, the ones that had to wait included.
            if let Err(e) = note(store, ctx, "instruction", &instruction(&text, applied)) {
                outcome.failure = Some(format!("could not record the instruction: {}", e.message));
                return Some(Next::Ended);
            }
            let session = resume?;
            let launch = Launch::resume(ctx.id, &session, held);
            held.clear();
            // The diff so far is kept and nothing is reverted: it is the
            // session that carries the work forward, not the process.
            run.cancel();
            Some(Next::Restart(launch))
        }
    }
}

/// A row in the task log that no provider said - Orteca or the user did.
fn note(store: &Store, ctx: &Context, kind: &str, payload: &str) -> crate::error::Result<()> {
    store.append_event(ctx.task_id, STAGE, kind, ctx.id.program(), payload)?;
    Ok(())
}

fn instruction(text: &str, applied: &str) -> String {
    serde_json::json!({ "kind": "instruction", "data": { "text": text, "applied": applied } }).to_string()
}

/// Log the event, then show it. The log is the record; the emit is the view.
fn record(store: &Store, task_id: i64, id: ProviderId, event: &ProviderEvent) -> crate::error::Result<()> {
    let payload = serde_json::to_string(event).map_err(|e| crate::error::AppError::new(crate::error::ErrorKind::Invalid, e.to_string()))?;
    // ponytail: SQLite writes on the async runtime. They are single-row inserts
    // on a local file; move them to spawn_blocking if a run ever feels slow.
    store.append_event(task_id, STAGE, event.kind(), id.program(), &payload)?;
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

    #[test]
    fn deny_rules_are_individual_arguments_for_both_windows_shell_tools() {
        let argv = args(ProviderId::Claude, "test");
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
            task_id: store.create_task(project.id, "test", "balanced", None, None, false).unwrap(),
            id: ProviderId::Codex, program: dir.join("fake.cmd"), dir,
            prompt: "a\"b %PATH% & ^\n\\ --help".into(), base_commit: None, dirty_at_start: false, recording: None,
        }
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
        assert_eq!(result.summary, format!("{prompt}\n"));
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

    /// A live run spends the user's subscription once. What the CLI said has to
    /// come back for free afterwards, or every later test of this path bills
    /// them again - and the hand-written fixtures have already been wrong once.
    #[tokio::test]
    async fn a_paid_run_is_recorded_so_it_can_be_replayed_for_free() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "recording");
        let dir = request.dir.clone();
        let recording = dir.join("recordings").join("task.jsonl");
        request.recording = Some(recording.clone());
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
        answered.absorb(&ProviderEvent::Done { result: "shipped".into(), structured: None });
        answered.finished = true;
        answered.cancelled = true;
        assert_eq!(answered.status(), "done", "the run had already ended");

        let mut mid_turn = Outcome::default();
        mid_turn.absorb(&ProviderEvent::Done { result: "turn one".into(), structured: None });
        mid_turn.cancelled = true;
        assert!(mid_turn.done, "a turn did answer");
        assert_eq!(mid_turn.status(), "cancelled", "but the run was stopped mid-work");

        let mut stopped = Outcome::default();
        stopped.cancelled = true;
        // Terminating the job leaves no exit code at all, and that is expected.
        stopped.exited(ProviderId::Codex, None, &["killed".into()]);
        assert_eq!(stopped.failure, None);
        assert_eq!(stopped.status(), "cancelled");
    }

    /// Steering a live provider. The instruction has to land while the turn is
    /// still running, and the run must then wait for the extra turn instead of
    /// ending at the first result - closing stdin there would throw the
    /// instruction away after accepting it.
    ///
    /// Also the token arithmetic a steered run depends on: Claude reports usage
    /// per turn but cost as a session running total, so two turns of one output
    /// token each are two tokens, at the later cost and not the sum of both.
    #[tokio::test]
    async fn a_live_provider_is_steered_mid_turn_and_the_run_waits_for_the_answer() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "steer-live");
        request.id = ProviderId::Claude;
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        // Echoes each user message back as a turn, slowly enough that an
        // instruction can arrive before the turn it belongs to has ended.
        std::fs::write(dir.join("fake.js"), concat!(
            "let buf='',turn=0;process.stdin.setEncoding('utf8');",
            "process.stdin.on('data',d=>{buf+=d;const ls=buf.split('\\n');buf=ls.pop();",
            "for(const l of ls){if(!l.trim())continue;const text=JSON.parse(l).message.content;",
            "console.log(JSON.stringify({type:'assistant',message:{content:[{type:'text',text}]}}));",
            "const cost=++turn*0.01;",
            "setTimeout(()=>console.log(JSON.stringify({type:'result',subtype:'success',result:text,",
            "usage:{input_tokens:1,output_tokens:1},total_cost_usd:cost})),300);}});",
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
                // The first text means turn one is under way but not finished.
                while hearing.recv().await != Some("text") {}
                live.send(task, Control::Instruct { text: "also tidy up".into(), apply_now: false }).unwrap();
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        assert_eq!(result.summary, "also tidy up", "the second turn never ran");
        let usage = result.usage.expect("a steered run still reports usage");
        assert_eq!(usage.output_tokens, 2, "each turn's tokens have to add up");
        assert_eq!(usage.cost_usd, Some(0.02), "cost is a session total, not a sum");

        let payload = store.event_payloads(task).into_iter()
            .find(|v| v["kind"] == "instruction")
            .expect("the instruction was not written to the log");
        assert_eq!(payload["data"]["applied"], "live");
        assert_eq!(payload["data"]["text"], "also tidy up");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A checkpoint provider has no stdin to speak down, so an instruction
    /// waits. It still has to be written to the log the moment it is given -
    /// an instruction that silently evaporates is the failure this guards.
    #[tokio::test]
    async fn a_checkpoint_provider_holds_an_instruction_it_cannot_take() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "steer-held");
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        // Talks, waits long enough to be steered, then finishes by itself.
        std::fs::write(dir.join("fake.js"), concat!(
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'working'}}));",
            "setTimeout(()=>{console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:1,output_tokens:1}}));},700);",
        )).unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| { let _ = heard.send(e.kind()); Ok(()) }),
            async {
                while hearing.recv().await != Some("text") {}
                live.send(task, Control::Instruct { text: "use tabs".into(), apply_now: false }).unwrap();
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
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
                while hearing.recv().await != Some("text") {}
                live.send(task, Control::Instruct { text: "make it faster".into(), apply_now: true }).unwrap();
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

    /// Apply-now before the provider has said who it is. There is no session to
    /// resume yet, so the instruction has to wait rather than look applied.
    #[tokio::test]
    async fn applying_now_without_a_session_yet_holds_instead_of_pretending() {
        let store = Store::in_memory().unwrap();
        let request = task_request(&store, "steer-nosession");
        let dir = request.dir.clone();
        let task = request.task_id;
        std::fs::write(&request.program, "@echo off\r\nnode \"%~dp0fake.js\"\r\n").unwrap();
        // Never reports a thread id at all.
        std::fs::write(dir.join("fake.js"), concat!(
            "console.log(JSON.stringify({type:'item.completed',item:{type:'agent_message',text:'anonymous'}}));",
            "setTimeout(()=>console.log(JSON.stringify({type:'turn.completed',usage:{input_tokens:1}})),700);",
        )).unwrap();

        let live = Live::default();
        let (heard, mut hearing) = tokio::sync::mpsc::unbounded_channel();
        let (result, ()) = tokio::join!(
            stream(&store, &live, request, move |e| { let _ = heard.send(e.kind()); Ok(()) }),
            async {
                while hearing.recv().await != Some("text") {}
                live.send(task, Control::Instruct { text: "hurry".into(), apply_now: true }).unwrap();
            }
        );

        assert_eq!(result.status, "done", "{:?}", result.failure);
        let payload = store.event_payloads(task).into_iter()
            .find(|v| v["kind"] == "instruction").expect("not logged");
        assert_eq!(payload["data"]["applied"], "held", "there was no session to resume");
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
                assert!(!args(id, prompt).contains(&prompt.to_string()), "{id:?}: {prompt:?}");
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
        let claude = args(ProviderId::Claude, "do the thing").join(" ");
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

        let codex = args(ProviderId::Codex, "do the thing").join(" ");
        assert!(codex.contains("--sandbox workspace-write"));
        assert!(!codex.contains("danger-full-access"));
        // The prompt is one argument, never spliced into a shell string.
        assert!(args(ProviderId::Codex, "a b").contains(&"-".to_string()));
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
