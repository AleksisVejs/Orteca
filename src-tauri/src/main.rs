#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod error;
mod proc;
mod project;
mod providers;
mod run;
mod store;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use error::{AppError, ErrorKind, Result};
use proc::Line;
use project::{GitState, TrustFinding};
use providers::{Detected, ProviderId};
use store::{Project, Store};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenedProject {
    project: Project,
    git: GitState,
    /// Known configuration sources and scan limitations, including ancestor paths.
    /// The UI requires consent for every untrusted project, even with no findings.
    trust_findings: Vec<TrustFinding>,
}

#[tauri::command]
fn open_project(path: String, store: State<Store>) -> Result<OpenedProject> {
    let dir = project::validate_dir(&path)?;
    let git = project::git_state(&dir);

    if !git.is_repo {
        return Err(AppError::new(
            ErrorKind::NotAGitRepo,
            "Orteca needs a Git repository so it can show you exactly what changed.",
        ));
    }

    // Anchor on the repo root, not wherever the user happened to point.
    let root_path = project::validate_dir(git.root.as_deref().unwrap_or(&path))?;
    let root = root_path.to_string_lossy().into_owned();

    let record = store.touch_project(&root, &project::display_name(&root_path))?;
    Ok(OpenedProject {
        trust_findings: project::trust_scan(&root_path),
        git,
        project: record,
    })
}

/// Live, never stored: a CLI can be installed or signed in while Orteca runs.
/// Returns a row per provider - "not installed" is a state the UI shows, not
/// an error, since the app is expected to run with neither CLI present.
#[tauri::command]
async fn detect_providers() -> Result<Vec<Detected>> {
    tauri::async_runtime::spawn_blocking(|| ProviderId::ALL.into_iter().map(ProviderId::detect).collect()).await
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))
}

/// Start one single-stage run and return its task id. The stream arrives as
/// `task-event` payloads and ends with one `task-done`.
#[tauri::command]
async fn start_task(
    app: AppHandle,
    path: String,
    prompt: String,
    provider: ProviderId,
    events: tauri::ipc::Channel<providers::ProviderEvent>,
) -> Result<run::TaskResult> {
    let prepare_app = app.clone();
    let request = tauri::async_runtime::spawn_blocking(move || prepare_run(&prepare_app.state::<Store>(), path, prompt, provider)).await
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))??;
    Ok(run::stream(&app.state::<Store>(), request, |event| events.send(event.clone())
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))).await)
}

fn prepare_run(store: &Store, path: String, prompt: String, provider: ProviderId) -> Result<run::Request> {
    let Some(prompt) = run::clean_prompt(&prompt) else {
        return Err(AppError::new(ErrorKind::Invalid, "Type what you want done first."));
    };

    let (dir, record) = trusted_dir(store, &path)?;

    let Some(program) = providers::which(provider.program()) else {
        return Err(AppError::new(
            ErrorKind::CliMissing,
            format!("{} is not installed or not on PATH.", provider.program()),
        ));
    };

    // The baseline is taken before the agent runs, so the diff afterwards has
    // something honest to compare against.
    let git = project::git_state(&dir);
    let task_id = store.create_task(
        record.id,
        &prompt,
        "balanced",
        git.branch.as_deref(),
        git.head.as_deref(),
        git.dirty,
    )?;

    Ok(run::Request {
        task_id,
        id: provider,
        program,
        dir,
        prompt,
        base_commit: git.head,
        dirty_at_start: git.dirty,
    })
}

fn trusted_dir(store: &Store, path: &str) -> Result<(std::path::PathBuf, Project)> {
    let dir = project::validate_dir(path)?;
    let record = store.project(&dir.to_string_lossy())?;
    if !record.trusted {
        return Err(AppError::new(ErrorKind::NotTrusted, "This project has not been trusted yet."));
    }
    Ok((dir, record))
}

/// Install a provider CLI with npm, streaming its output as `install-event`.
/// Orteca never fetches a binary itself - it runs the package manager the user
/// already has, and a failed install is reported rather than left as a button
/// that appears to do nothing.
#[tauri::command(async)]
async fn install_provider(app: AppHandle, provider: ProviderId) -> Result<Detected> {
    let npm = providers::which("npm").ok_or_else(|| {
        AppError::new(
            ErrorKind::CliMissing,
            "npm is not on PATH. Install Node.js first, then try again.",
        )
    })?;
    npm_install(provider, &npm, |line| { let _ = app.emit("install-event", (provider, line)); }).await?;

    let detected = tauri::async_runtime::spawn_blocking(move || provider.detect()).await
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
    if detected.path.is_none() {
        return Err(AppError::new(
            ErrorKind::CliMissing,
            format!(
                "{} installed, but npm's global folder is not on PATH yet. Restart Orteca.",
                provider.program()
            ),
        ));
    }
    Ok(detected)
}

/// Spawn a tool, stream every line to the UI as it arrives, and wait for it to
/// exit. Returns the exit code and the last few lines, because both npm and the
/// provider CLIs explain a failure on the way down rather than at the end.
///
/// Always runs in the temp directory, never the user's project: a repository's
/// own `.npmrc` can redirect the registry and its `.claude/settings.json` hooks
/// run on CLI startup - and this happens before anyone has consented to it.
async fn stream_tool(
    program: &std::path::Path,
    args: &[&str],
    emit: impl Fn(String),
) -> Result<(Option<i32>, Vec<String>)> {
    let mut run = proc::spawn(&program.to_string_lossy(), args, &std::env::temp_dir())?;
    // Closed on purpose: nothing here can answer a prompt, so a CLI that wants
    // one must fail fast instead of hanging a button forever.
    run.close_stdin();

    let mut tail: Vec<String> = Vec::new();
    while let Some(line) = run.lines.recv().await {
        let text = match line {
            Line::Text(text) => text,
            Line::Json(value) => value.to_string(),
            Line::Exit(code) => return Ok((code, tail)),
        };
        if text.trim().is_empty() {
            continue;
        }
        emit(text.clone());
        tail.push(text);
        if tail.len() > 4 {
            tail.remove(0);
        }
    }
    Err(AppError::new(
        ErrorKind::Io,
        format!("{} stopped without reporting an exit code", program.display()),
    ))
}

async fn npm_install(provider: ProviderId, npm: &std::path::Path, emit: impl Fn(String)) -> Result<()> {
    static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _install = INSTALL.try_lock().map_err(|_| AppError::new(ErrorKind::Invalid, "Another provider installation is already running."))?;
    let package = provider.package();
    let (code, tail) = stream_tool(npm, &["install", "--global", package], emit).await?;
    if code == Some(0) {
        Ok(())
    } else {
        Err(AppError::new(
            ErrorKind::Io,
            format!("npm could not install {package}: {}", tail.join(" ")),
        ))
    }
}

/// Run the provider CLI's own sign-in and re-detect afterwards. Orteca opens no
/// login page and never sees a password: the CLI prints a URL, the user approves
/// it in their browser, and the CLI writes its own credential. All Orteca does
/// is start it, show the output, and ask the CLI again when it is done.
#[tauri::command(async)]
async fn sign_in_provider(app: AppHandle, provider: ProviderId) -> Result<Detected> {
    static SIGN_IN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _lock = SIGN_IN
        .try_lock()
        .map_err(|_| AppError::new(ErrorKind::Invalid, "A sign-in is already running."))?;

    let program = providers::which(provider.program()).ok_or_else(|| {
        AppError::new(
            ErrorKind::CliMissing,
            format!("{} is not installed or not on PATH.", provider.program()),
        )
    })?;

    // A browser round trip is slow but not unbounded. Without this the button
    // would hang forever on a login the user abandoned - cancel arrives in M5.
    let stream = stream_tool(&program, provider.login_args(), |line| {
        let _ = app.emit("sign-in-event", (provider, line));
    });
    let (code, tail) = tokio::time::timeout(std::time::Duration::from_secs(300), stream)
        .await
        .map_err(|_| {
            AppError::new(
                ErrorKind::Io,
                format!("{} sign-in timed out after 5 minutes.", provider.program()),
            )
        })??;

    let detected = tauri::async_runtime::spawn_blocking(move || provider.detect())
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;

    // The CLI's own answer decides, not its exit code: a sign-in that reports
    // success but left the CLI signed out is still signed out.
    if detected.auth == providers::Auth::Subscription || detected.auth == providers::Auth::ApiKey {
        return Ok(detected);
    }
    Err(AppError::new(
        ErrorKind::Invalid,
        if tail.is_empty() {
            format!("{} is still signed out (exit {code:?}).", provider.program())
        } else {
            format!("{} is still signed out: {}", provider.program(), tail.join(" "))
        },
    ))
}

#[tauri::command]
fn recent_projects(store: State<Store>) -> Result<Vec<Project>> {
    store.recent_projects(8)
}

#[tauri::command]
fn trust_project(path: String, trusted: bool, store: State<Store>) -> Result<()> {
    store.set_trusted(&path, trusted)
}

#[tauri::command]
fn forget_project(path: String, store: State<Store>) -> Result<()> {
    store.forget_project(&path)
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let db = app.path().app_data_dir()?.join("orteca.db");
            app.manage(Store::open(&db).map_err(|e| e.message)?);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_project,
            detect_providers,
            install_provider,
            sign_in_provider,
            recent_projects,
            start_task,
            trust_project,
            forget_project
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Orteca");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_install_requests_are_rejected_on_the_backend() {
        let root = std::env::temp_dir().join(format!("orteca-install-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let fake = root.join("fake.cmd");
        std::fs::write(&fake, "@echo off\r\necho fake installer\r\nping -n 2 127.0.0.1 >nul\r\nexit /b 0\r\n").unwrap();
        let (first, second) = tokio::join!(npm_install(ProviderId::Codex, &fake, |_| {}), npm_install(ProviderId::Claude, &fake, |_| {}));
        assert!(first.is_ok());
        assert!(second.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trust_is_bound_to_the_canonical_directory() {
        let root = std::env::temp_dir().join(format!("orteca-run-trust-{}", std::process::id()));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let canonical = root.canonicalize().unwrap();
        let store = Store::in_memory().unwrap();
        let key = canonical.to_str().unwrap();
        store.touch_project(key, "test").unwrap();
        assert!(trusted_dir(&store, key).is_err());
        store.set_trusted(key, true).unwrap();
        for path in [root.clone(), root.join("."), root.join("sub/.."), std::path::PathBuf::from(root.to_string_lossy().to_uppercase())] {
            let (dir, _) = trusted_dir(&store, path.to_str().unwrap()).unwrap();
            assert_eq!(dir, canonical);
        }
        assert!(trusted_dir(&store, root.join("sub").to_str().unwrap()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
