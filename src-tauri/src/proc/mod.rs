//! Running an agent CLI: spawn it inside a Job Object, read its JSONL stream,
//! keep stdin open for steering, kill the whole tree on cancel.
//!
//! Provider-agnostic on purpose — `providers::claude` and `providers::codex`
//! both emit newline-delimited JSON, they just disagree about the field names.

mod job;

use std::io;
use std::path::Path;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::mpsc;

use job::Job;

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
    pub lines: mpsc::Receiver<Line>,
    stdin: Option<ChildStdin>,
    _job: Job,
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
}

/// Spawn `program` with `args` in `cwd`, inside a fresh Job Object.
pub fn spawn(program: &str, args: &[&str], cwd: &Path) -> io::Result<Run> {
    let job = Job::new()?;

    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;

    let pid = child
        .id()
        .ok_or_else(|| io::Error::other("child exited before it could be adopted"))?;
    job.assign(pid)?;

    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // 256 is generous: a chatty agent emits a few hundred events per run, and a
    // full channel back-pressures the reader rather than dropping events.
    let (tx, rx) = mpsc::channel(256);

    let out_tx = tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let event = match serde_json::from_str::<Value>(&line) {
                Ok(v) => Line::Json(v),
                Err(_) => Line::Text(line),
            };
            if out_tx.send(event).await.is_err() {
                return;
            }
        }
    });

    let err_tx = tx.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if err_tx.send(Line::Text(line)).await.is_err() {
                return;
            }
        }
    });

    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|s| s.code());
        let _ = tx.send(Line::Exit(code)).await;
    });

    Ok(Run {
        lines: rx,
        stdin,
        _job: job,
    })
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
        panic!("grandchild {grandchild} survived cancel — job object is not working");
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
}
