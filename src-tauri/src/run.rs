//! A single-stage run: prompt -> one provider CLI -> stream -> diff -> result.
//!
//! Routing, extra stages and steering are later milestones. What is here is the
//! whole honest path for one task: build the argv, spawn inside the Job Object,
//! normalise every line through the provider's parser, write each event to the
//! append-only log, and end with a diff the user can check.

use std::path::PathBuf;

use serde::Serialize;

use crate::proc::{self, Line};
use crate::project::{self, FileStat};
use crate::providers::{ProviderEvent, ProviderId, Usage};
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

/// What the UI gets when the run ends.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub task_id: i64,
    /// `done` or `failed`.
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
    /// Last wins. A single-stage `exec` run reports usage once; if several
    /// stages ever share one Outcome, this has to become a sum.
    usage: Option<Usage>,
    failure: Option<String>,
    done: bool,
}

impl Outcome {
    fn exited(&mut self, id: ProviderId, code: Option<i32>, noise: &[String]) {
        if self.failure.is_none() && (code != Some(0) || !self.done) {
            self.failure = Some(exit_message(id, code, noise));
        }
    }

    fn absorb(&mut self, event: &ProviderEvent) {
        match event {
            ProviderEvent::Text(text) => self.last_text = text.clone(),
            ProviderEvent::Usage(usage) => self.usage = Some(usage.clone()),
            ProviderEvent::Done { result, .. } => {
                self.done = true;
                self.result = result.clone();
            }
            ProviderEvent::Failed { message, .. } => self.failure = Some(message.clone()),
            _ => {}
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

pub async fn stream(store: &Store, request: Request, emit: impl Fn(&ProviderEvent) -> crate::error::Result<()>) -> TaskResult {
    let Request { task_id, id, program, dir, prompt, base_commit, dirty_at_start, recording } = request;
    let mut recording = Recording::new(recording);
    let mut outcome = Outcome::default();
    // Non-JSON output is the only clue a CLI leaves when it dies badly.
    let mut noise: Vec<String> = Vec::new();

    let argv = args(id, &prompt);
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    match proc::spawn(&program.to_string_lossy(), &borrowed, &dir) {
        Err(e) => outcome.failure = Some(format!("could not start {}: {e}", id.program())),
        Ok(mut run) => {
            // Prompt bytes bypass cmd.exe parsing and its command-line limit.
            if let Err(e) = run.send_line(&prompt).await {
                outcome.failure = Some(format!("could not send prompt to {}: {e}", id.program()));
            }
            run.close_stdin();
            while let Some(line) = run.lines.recv().await {
                match line {
                    Line::Json(value) => {
                        // Before parsing: the recording is what the CLI said,
                        // not what Orteca understood of it.
                        recording.write(&value);
                        let events = id.parse_line(&value);
                        // Each parser understands a subset of its CLI's event
                        // types and silently drops the rest. Harmless for the
                        // stream, fatal for the record: a Codex run whose tool
                        // calls all failed said so in an item type this parser
                        // does not know, and the log kept no trace of why the
                        // run did nothing. Keep the raw line. It is not emitted
                        // - the UI has no shape for it - so the log stays
                        // complete while the stream stays readable.
                        if events.is_empty() {
                            if let Err(e) = store.append_event(task_id, STAGE, "unknown", id.program(), &value.to_string()) {
                                outcome.failure = Some(format!("could not record run event: {}", e.message));
                                break;
                            }
                        }
                        for event in events {
                            if let Err(e) = record(store, task_id, id, &event).and_then(|()| emit(&event)) {
                                outcome.failure = Some(format!("could not record or deliver run event: {}", e.message));
                                break;
                            }
                            outcome.absorb(&event);
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
                        outcome.exited(id, code, &noise);
                        break;
                    }
                }
            }
        }
    }

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
    let mut status = if outcome.failure.is_some() {
        "failed"
    } else {
        "done"
    };
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
        let result = stream(&store, request, |e| { events.borrow_mut().push(e.clone()); Ok(()) }).await;
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
        stream(&store, request, |e| { emitted.borrow_mut().push(e.kind()); Ok(()) }).await;

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
        let result = stream(&store, request, |e| {
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

    #[tokio::test]
    async fn spawn_failure_is_returned_and_logged() {
        let store = Store::in_memory().unwrap();
        let mut request = task_request(&store, "spawn-failure");
        request.program = request.dir.join("missing.exe");
        let dir = request.dir.clone();
        let task = request.task_id;
        let result = stream(&store, request, |_| Ok(())).await;
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
            let result = stream(&store, request, |_| {
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
