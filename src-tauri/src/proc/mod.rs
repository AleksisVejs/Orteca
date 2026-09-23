//! Running an agent CLI: spawn it inside a Job Object, read its JSONL stream,
//! keep stdin open for steering, kill the whole tree on cancel.
//!
//! Provider-agnostic on purpose - `providers::claude` and `providers::codex`
//! both emit newline-delimited JSON, they just disagree about the field names.

pub mod job;

use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

pub use job::Job;

/// One line off a running CLI.
#[derive(Debug, Clone)]
pub enum Line {
    /// A parsed JSONL event from stdout.
    Json(Value),
    /// stdout that was not JSON, or anything on stderr.
    Text(String),
    /// Process ended. `None` if it was killed by a signal or by cancel.
    Exit(Option<i32>),
}

/// A live CLI process. Dropping this kills the whole tree.
pub struct Run {
    pub lines: mpsc::UnboundedReceiver<Line>,
    stdin: Option<ChildStdin>,
    job: Arc<Job>,
}

impl Drop for Run {
    fn drop(&mut self) {
        // The exit waiter also owns the job, so cancellation cannot wait for last close.
        let _ = self.job.terminate();
    }
}

impl Run {
    /// Push a line into the process's stdin. This is how a mid-task instruction
    /// reaches a running Claude session (`--input-format stream-json`).
    /// Codex has no stdin channel once started; its provider returns
    /// `Steering::Checkpoint` and never calls this.
    pub async fn send_line(&mut self, line: &str) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("process stdin is closed"))?;
        stdin.write_all(line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await
    }

    /// Close stdin so the CLI knows no more input is coming.
    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Kill the whole tree now, without giving up the `Run`. Dropping it would
    /// do the same killing but also close the receiver, and a stopped run still
    /// has something to say: the exit waiter delivers what the CLI had already
    /// written, then `Line::Exit`, so the caller ends on a drained stream
    /// rather than on a truncated one.
    pub fn cancel(&self) {
        let _ = self.job.terminate();
    }
}

/// Spawn `program` with `args` in `cwd`, inside a fresh Job Object.
pub fn spawn(program: &str, args: &[&str], cwd: &Path) -> io::Result<Run> {
    spawn_env(program, args, cwd, &[])
}

/// `spawn`, with these variables added to the inherited environment.
pub fn spawn_env(program: &str, args: &[&str], cwd: &Path, env: &[(&str, PathBuf)]) -> io::Result<Run> {
    let job = Arc::new(Job::new()?);
    let mut child = spawn_suspended(program, args, cwd, env)?;

    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("child exited before it could be adopted"))?;
    job.assign(pid)?;
    job::resume(pid)?;

    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // A paused consumer must not prevent the process from draining its pipes.
    // ponytail: output is buffered in memory; spool to disk if large runs need a cap.
    let (tx, rx) = mpsc::unbounded_channel();

    let out_tx = tx.clone();
    let out = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        while let Ok(size) = reader.read_line(&mut line).await {
            if size == 0 {
                break;
            }
            // EOF can leave valid-looking JSON that was never a complete JSONL event.
            let complete = line.ends_with('\n');
            if complete {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            let event = match complete.then(|| serde_json::from_str::<Value>(&line)) {
                Some(Ok(v)) => Line::Json(v),
                _ => Line::Text(line.clone()),
            };
            if out_tx.send(event).is_err() {
                return;
            }
            line.clear();
        }
    });

    let err_tx = tx.clone();
    let err = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if err_tx.send(Line::Text(line)).is_err() {
                return;
            }
        }
    });

    let exit_job = Arc::clone(&job);
    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|s| s.code());
        // Detached descendants can otherwise keep inherited output pipes open forever.
        let _ = exit_job.terminate();
        let _ = out.await;
        let _ = err.await;
        let _ = tx.send(Line::Exit(code));
    });

    Ok(Run {
        lines: rx,
        stdin,
        job,
    })
}

/// A temp directory Orteca owns, for every child it spawns.
///
/// Codex's Windows sandbox grants a write ACE on each of its write roots, and
/// TEMP is one of them. When TEMP points somewhere the user cannot re-ACL -
/// `E:\Temp` owned by BUILTIN\Administrators grants Modify, which does not
/// include WRITE_DAC - `SetNamedSecurityInfoW` fails with ERROR_ACCESS_DENIED,
/// the sandbox refuses to start, and the agent loses every tool while the run
/// still exits 0. The inherited TEMP is not Orteca's to repair; the child's is.
///
/// One stable directory rather than one per run: Codex caches its runtimes
/// under TEMP, and a fresh directory would re-pay that on every task.
pub fn owned_temp() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
        .join("app.orteca")
        .join("tmp");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Whether this account may rewrite `dir`'s ACL, which Codex's Windows sandbox
/// does to every write root, the workspace included. `owned_temp` fixes TEMP;
/// a repository under a directory like `E:\Temp` cannot be fixed from here, and
/// Codex there exits 0 having had no tools at all. Asking for a handle with
/// WRITE_DAC changes nothing and fails exactly where the sandbox would.
pub fn can_change_acl(dir: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const WRITE_DAC: u32 = 0x0004_0000;
    // A directory handle needs backup semantics.
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    std::fs::OpenOptions::new()
        .access_mode(WRITE_DAC)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)
        .is_ok()
}

fn spawn_suspended(program: &str, args: &[&str], cwd: &Path, env: &[(&str, PathBuf)]) -> io::Result<Child> {
    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(
            (windows::Win32::System::Threading::CREATE_SUSPENDED
                | windows::Win32::System::Threading::CREATE_NO_WINDOW)
                .0,
        )
        .kill_on_drop(true);
    // Inherit the user's TEMP when Orteca has nowhere better; a missing
    // LOCALAPPDATA is not a reason to refuse to run.
    if let Some(temp) = owned_temp() {
        command.env("TEMP", &temp).env("TMP", &temp);
    }
    command.spawn()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn is_alive(pid: u32) -> bool {
        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code).is_ok();
            let _ = CloseHandle(handle);
            ok && code == STILL_ACTIVE.0 as u32
        }
    }

    #[tokio::test]
    async fn child_cannot_execute_before_job_assignment() {
        let mut child = spawn_suspended(
            "node",
            &["-e", "console.log('escaped')"],
            &std::env::temp_dir(),
            &[],
        )
        .unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reader.read_line(&mut line),
        )
        .await;
        child.kill().await.unwrap();
        assert!(
            output.is_err(),
            "child executed before job assignment: {line}"
        );
    }

    #[tokio::test]
    async fn exit_is_reported_when_descendant_keeps_pipes_open() {
        let mut run = spawn("node", &["-e", "const c=require('child_process').spawn(process.execPath,['-e','setInterval(()=>{},1000)'],{detached:true,stdio:['ignore',process.stdout,process.stderr]});console.log(JSON.stringify({pid:c.pid}));c.unref()"], &std::env::temp_dir()).unwrap();
        let mut descendant = None;
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(line) = run.lines.recv().await {
                match line {
                    Line::Json(v) => descendant = v["pid"].as_u64().map(|pid| pid as u32),
                    Line::Exit(code) => {
                        assert_eq!(code, Some(0));
                        return;
                    }
                    _ => {}
                }
            }
            panic!("missing exit");
        })
        .await;
        drop(run);
        assert!(result.is_ok(), "descendant pipes hid the parent's exit");
        assert!(
            !is_alive(descendant.unwrap()),
            "descendant survived parent exit"
        );
    }

    /// The whole point of the Job Object: cancelling must take the grandchild
    /// with it, not just the process we spawned.
    #[tokio::test]
    async fn cancel_kills_the_grandchild() {
        // node (child) spawns a detached node (grandchild) and reports its pid.
        let script = "const c=require('child_process').spawn(process.execPath,\
            ['-e','setInterval(()=>{},1000)'],{detached:true,stdio:'ignore'});\
            console.log(JSON.stringify({pid:c.pid}));setInterval(()=>{},1000);";

        let cwd = std::env::temp_dir();
        let mut run = spawn("node", &["-e", script], &cwd).expect("spawn");

        let grandchild = loop {
            match run.lines.recv().await.expect("child produced no pid") {
                Line::Json(v) => break v["pid"].as_u64().expect("pid field") as u32,
                Line::Exit(code) => panic!("child exited early: {code:?}"),
                Line::Text(_) => continue,
            }
        };
        assert!(is_alive(grandchild), "grandchild should be running");

        drop(run); // cancel

        // KILL_ON_JOB_CLOSE is not instant; give the kernel a moment.
        for _ in 0..40 {
            if !is_alive(grandchild) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("grandchild {grandchild} survived cancel - job object is not working");
    }

    #[tokio::test]
    async fn reads_jsonl_and_reports_exit() {
        let cwd = std::env::temp_dir();
        let mut run = spawn(
            "node",
            &["-e", "console.log('{\"a\":1}');console.log('not json')"],
            &cwd,
        )
        .expect("spawn");

        let mut saw_json = false;
        let mut saw_text = false;
        let mut exit = None;
        while let Some(line) = run.lines.recv().await {
            match line {
                Line::Json(v) => saw_json = v["a"] == 1,
                Line::Text(t) if t == "not json" => saw_text = true,
                Line::Exit(code) => exit = Some(code),
                Line::Text(_) => {}
            }
        }
        assert!(saw_json, "JSON line should parse");
        assert!(saw_text, "non-JSON line should pass through as text");
        assert_eq!(exit, Some(Some(0)));
    }

    #[tokio::test]
    async fn exit_follows_all_output() {
        let mut run = spawn(
            "node",
            &["-e", "for(let i=0;i<2000;i++)console.log('{}')"],
            &std::env::temp_dir(),
        )
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let mut count = 0;
        while let Some(line) = run.lines.recv().await {
            match line {
                Line::Json(_) => count += 1,
                Line::Exit(_) => {
                    assert_eq!(count, 2000);
                    return;
                }
                _ => {}
            }
        }
        panic!("no exit");
    }

    #[tokio::test]
    async fn unterminated_json_is_text() {
        let mut run = spawn(
            "node",
            &["-e", "process.stdout.write('{}')"],
            &std::env::temp_dir(),
        )
        .unwrap();
        let mut saw_text = false;
        while let Some(line) = run.lines.recv().await {
            match line {
                Line::Json(_) => panic!("unterminated line was parsed as JSON"),
                Line::Text(t) => saw_text = t == "{}",
                _ => {}
            }
        }
        assert!(saw_text);
    }

    #[tokio::test]
    async fn steering_and_cancel_mid_write() {
        let mut run = spawn("node", &["-e", "process.stdin.once('data',d=>{console.log(JSON.stringify({input:d.toString().trim()}));process.stdout.write('{\"partial\":');console.error('ready')});setInterval(()=>{},1000)"], &std::env::temp_dir()).unwrap();
        run.send_line("continue").await.unwrap();
        let mut steered = false;
        loop {
            let line = tokio::time::timeout(std::time::Duration::from_secs(5), run.lines.recv())
                .await
                .unwrap()
                .unwrap();
            match line {
                Line::Json(v) => steered = v["input"] == "continue",
                Line::Text(t) if t == "ready" => break,
                _ => {}
            }
        }
        run.cancel();
        run.close_stdin();
        let mut partial = false;
        while let Some(line) =
            tokio::time::timeout(std::time::Duration::from_secs(5), run.lines.recv())
                .await
                .unwrap()
        {
            match line {
                Line::Json(v) => {
                    assert_eq!(v["input"], "continue");
                    steered = true;
                }
                Line::Text(t) => partial |= t == "{\"partial\":",
                _ => {}
            }
        }
        assert!(steered && partial);
    }

    #[tokio::test]
    async fn paused_consumer_does_not_stall_process() {
        let mut run = spawn("node", &["-e", "console.log(JSON.stringify({pid:process.pid}));for(let i=0;i<10000;i++)console.log('{}')"], &std::env::temp_dir()).unwrap();
        let Some(Line::Json(first)) = run.lines.recv().await else {
            panic!("no pid")
        };
        let pid = first["pid"].as_u64().unwrap() as u32;
        for _ in 0..100 {
            if !is_alive(pid) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("paused consumer blocked process exit");
    }

    /// A provider that has to re-ACL its own TEMP cannot do it when TEMP is a
    /// directory the user only has Modify on. Codex fails that way and then
    /// exits 0 with every tool gone, so the child gets a directory Orteca owns.
    #[tokio::test]
    async fn children_run_with_a_temp_directory_orteca_owns() {
        let expected = owned_temp().expect("LOCALAPPDATA");
        let mut run = spawn(
            "node",
            &[
                "-e",
                "console.log(JSON.stringify({temp:process.env.TEMP,tmp:process.env.TMP}))",
            ],
            &std::env::temp_dir(),
        )
        .unwrap();

        let mut reported = None;
        while let Some(line) = run.lines.recv().await {
            if let Line::Json(v) = line {
                reported = Some((
                    v["temp"].as_str().unwrap_or_default().to_string(),
                    v["tmp"].as_str().unwrap_or_default().to_string(),
                ));
            }
        }
        let (temp, tmp) = reported.expect("child reported no temp");
        assert_eq!(
            std::path::Path::new(&temp),
            expected,
            "child inherited a TEMP Orteca does not own"
        );
        assert_eq!(
            std::path::Path::new(&tmp),
            expected,
            "TMP and TEMP must agree"
        );

        // The whole point: this directory has to accept a write.
        let probe = expected.join("write-probe");
        std::fs::write(&probe, "x").expect("owned temp must be writable");
        std::fs::remove_file(&probe).unwrap();
    }

    /// An OWNER RIGHTS ACE granting Modify strips the owner's implicit
    /// WRITE_DAC, which is the `E:\Temp` shape without needing an admin.
    #[test]
    fn a_directory_whose_acl_cannot_be_changed_is_detected() {
        let dir = std::env::temp_dir().join(format!("orteca-acl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(can_change_acl(&dir));
        let user = std::env::var("USERNAME").unwrap();
        let status = std::process::Command::new("icacls")
            .arg(&dir)
            .args(["/inheritance:r", "/grant", "*S-1-3-4:(OI)(CI)M", "/grant"])
            .arg(format!("{user}:(OI)(CI)M"))
            .output()
            .unwrap()
            .status;
        assert!(status.success());
        assert!(!can_change_acl(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn assigning_an_invalid_process_fails() {
        assert!(Job::new().unwrap().assign(0).is_err());
    }
}
