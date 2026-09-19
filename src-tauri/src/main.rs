#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::collections::HashMap;
use std::sync::Mutex;

mod codemap;
mod error;
mod intent;
mod proc;
mod project;
mod providers;
mod routing;
mod run;
mod store;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use error::{AppError, ErrorKind, Result};
use proc::Line;
use project::{GitState, Isolation, TrustFinding};
use providers::{Detected, ProviderId};
use routing::{Mode, Route};
use store::{NewTask, Project, Store};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenedProject {
    project: Project,
    git: GitState,
    /// Known configuration sources and scan limitations, including ancestor paths.
    /// The UI requires consent for every untrusted project, even with no findings.
    trust_findings: Vec<TrustFinding>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preflight {
    provider: ProviderId,
    git: GitState,
    route: Route,
    /// What the implementation asks this provider for, including route overrides.
    model: routing::ModelChoice,
    /// What Plan runs on when Efficient mode lowers its reasoning effort.
    plan: Option<routing::ModelChoice>,
    /// What the Review runs on, when that is not the route's tier.
    review: Option<routing::ModelChoice>,
}

struct PlannedRun {
    dir: std::path::PathBuf,
    project: Project,
    program: std::path::PathBuf,
    git: GitState,
    prompt: String,
    route: Route,
}

/// One operation per provider. Cancellation is separate from the process
/// runner because installs and sign-ins happen outside a trusted repository.
#[derive(Default)]
struct ProviderOperations(Mutex<HashMap<ProviderId, tokio::sync::watch::Sender<bool>>>);

impl ProviderOperations {
    fn begin(&self, provider: ProviderId) -> Result<tokio::sync::watch::Receiver<bool>> {
        let mut operations = self.0.lock().expect("provider operations poisoned");
        if operations.contains_key(&provider) {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "A provider operation is already running.",
            ));
        }
        let (sender, receiver) = tokio::sync::watch::channel(false);
        operations.insert(provider, sender);
        Ok(receiver)
    }

    fn cancel(&self, provider: ProviderId) -> Result<()> {
        let sender = self
            .0
            .lock()
            .expect("provider operations poisoned")
            .get(&provider)
            .cloned()
            .ok_or_else(|| {
                AppError::new(
                    ErrorKind::NotFound,
                    "That provider operation has already finished.",
                )
            })?;
        sender.send(true).map_err(|_| {
            AppError::new(
                ErrorKind::NotFound,
                "That provider operation has already finished.",
            )
        })
    }

    fn finish(&self, provider: ProviderId) {
        self.0
            .lock()
            .expect("provider operations poisoned")
            .remove(&provider);
    }
}

#[tauri::command]
fn open_project(app: AppHandle, path: String, store: State<Store>) -> Result<OpenedProject> {
    let dir = project::validate_dir(&path)?;
    let git = project::git_state(&dir);

    if !git.is_repo {
        if !project::git_installed() {
            return Err(AppError::new(
                ErrorKind::NotFound,
                "Git is not installed or not on PATH. Install Git for Windows, then open the project again.",
            ));
        }
        return Err(AppError::new(
            ErrorKind::NotAGitRepo,
            "Orteca needs a Git repository so it can show you exactly what changed.",
        ));
    }

    // Anchor on the repo root, not wherever the user happened to point.
    let root_path = project::validate_dir(git.root.as_deref().unwrap_or(&path))?;
    let root = root_path.to_string_lossy().into_owned();

    let record = store.touch_project(&root, &project::display_name(&root_path))?;
    // A first scan of a large repository takes seconds; the screen must not wait.
    let (id, dir) = (record.id, root_path.clone());
    std::thread::spawn(move || {
        let _ = app.state::<Store>().scan_map(id, &dir, &project::tracked_paths(&dir));
    });
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
async fn detect_providers(found: tauri::ipc::Channel<Detected>) -> Result<Vec<Detected>> {
    // Every provider at once, and each row is sent the moment it lands. Asking
    // them one after another spent one CLI's start-up waiting on the previous
    // one's, and the card stayed empty until the slowest had answered.
    let probes: Vec<_> = ProviderId::ALL
        .into_iter()
        .map(|id| {
            let found = found.clone();
            tauri::async_runtime::spawn(async move {
                let one = id.detect_async().await;
                // A closed channel is the window going away, not a failed
                // detection: the response still carries the full set.
                let _ = found.send(one.clone());
                one
            })
        })
        .collect();

    let mut detected = Vec::with_capacity(probes.len());
    for probe in probes {
        detected.push(
            probe
                .await
                .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?,
        );
    }
    Ok(detected)
}

/// Start one single-stage run. Events arrive on `events` and the response is
/// the finished result; `task` carries the task id the moment the row exists,
/// which is what `cancel_task` needs and what makes a run stoppable long
/// before it resolves.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn start_task(
    app: AppHandle,
    path: String,
    prompt: String,
    provider: ProviderId,
    mode: Mode,
    headroom: Option<f64>,
    isolation: Isolation,
    attachments: Vec<String>,
    resume: Option<run::Resume>,
    continue_task: Option<i64>,
    events: tauri::ipc::Channel<providers::ProviderEvent>,
    task: tauri::ipc::Channel<i64>,
    checking: tauri::ipc::Channel<run::TaskResult>,
) -> Result<run::TaskResult> {
    // A session id goes into argv, so it is an id and nothing else. A copy is
    // a new folder, where neither CLI can find the session.
    let resume = match resume {
        Some(r) if isolation == Isolation::CurrentTree && !r.reply.trim().is_empty() => {
            let id_like = !r.session.is_empty()
                && !r.session.starts_with('-')
                && r.session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            if !id_like {
                return Err(AppError::new(ErrorKind::Invalid, "That session id is not one a CLI gave."));
            }
            Some(r)
        }
        _ => None,
    };
    // Checked before anything is recorded: a path that is gone would only
    // surface as an agent failing to read it.
    let attachments = attachments
        .iter()
        .map(|path| {
            let p = std::path::PathBuf::from(path);
            if p.is_absolute() && p.exists() {
                Ok(p)
            } else {
                Err(AppError::new(ErrorKind::Invalid, format!("Attachment not found: {path}")))
            }
        })
        .collect::<Result<Vec<_>>>()?;
    // Beside the database, because a recording belongs to the run it came from.
    // Losing the directory costs a replay, never the run itself.
    let recordings = app
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("recordings"));
    let mut request = begin(
        &app.state::<Store>(),
        recordings,
        path,
        prompt,
        provider,
        mode,
        headroom,
        isolation,
        continue_task,
    )
    .await?;
    request.attachments = attachments;
    request.resume = resume;
    request.checking = Some(Box::new(move |result| {
        let _ = checking.send(result.clone());
    }));
    // A closed channel is the window going away, not a reason to abandon a run
    // that is already recorded; the result still comes back to whoever asked.
    let _ = task.send(request.task_id);
    Ok(run::stream(
        &app.state::<Store>(),
        &app.state::<run::Live>(),
        request,
        |event| {
            events
                .send(event.clone())
                .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))
        },
    )
    .await)
}

/// Stop a running task and everything it spawned. Errors when the run has
/// already ended, because a Stop that silently does nothing is worse than one
/// that says it arrived too late.
#[tauri::command]
fn cancel_task(task_id: i64, live: State<run::Live>) -> Result<()> {
    live.send(task_id, run::Control::Cancel)
}

/// Give a running agent something more to work with.
///
/// Where it goes is the provider's business: Claude takes it mid-turn on
/// stdin, Codex has no channel and holds it. `apply_now` is the user choosing
/// not to wait - the process ends and its session is resumed carrying the
/// instruction, with the diff so far left exactly as it is.
#[tauri::command]
async fn send_instruction(
    task_id: i64,
    text: String,
    apply_now: bool,
    live: State<'_, run::Live>,
) -> Result<run::InstructionReceipt> {
    let Some(text) = run::clean_prompt(&text) else {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "Type the instruction first.",
        ));
    };
    live.instruct(task_id, text, apply_now).await
}

fn codex_acl_refusal() -> AppError {
    AppError::new(
        ErrorKind::Invalid,
        "Codex cannot sandbox this repository: Windows does not let your account change permissions on its folder, so every tool would fail while the run looked finished. Move the repository into a folder you own, such as your user folder, or run it with claude.",
    )
}

/// Everything a route is chosen from except the reading of the prompt, so the
/// classify call can run while this is gathered.
struct Scanned {
    dir: std::path::PathBuf,
    project: Project,
    program: std::path::PathBuf,
    git: GitState,
    prompt: String,
    mode: Mode,
    signals: routing::RepoSignals,
}

impl Scanned {
    fn route(self, intent: Option<intent::Intent>, job: Option<intent::Job>) -> PlannedRun {
        let signals = routing::RepoSignals {
            checks_locally: run::checks_locally(&self.dir, job),
            intent,
            job,
            ..self.signals
        };
        let route = routing::route(&self.prompt, self.mode, &signals);
        PlannedRun {
            dir: self.dir,
            project: self.project,
            program: self.program,
            git: self.git,
            prompt: self.prompt,
            route,
        }
    }
}

fn scan_run(
    store: &Store,
    path: String,
    prompt: String,
    provider: ProviderId,
    mode: Mode,
    headroom: Option<f64>,
    isolation: Isolation,
) -> Result<Scanned> {
    let Some(prompt) = run::clean_prompt(&prompt) else {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "Type what you want done first.",
        ));
    };
    let (dir, project) = trusted_dir(store, &path)?;
    let git = project::git_state(&dir);

    // A copy's folder does not exist yet; it is checked once it does.
    if provider == ProviderId::Codex
        && isolation == Isolation::CurrentTree
        && !proc::can_change_acl(&dir)
    {
        return Err(codex_acl_refusal());
    }
    if isolation == Isolation::Worktree && git.head.is_none() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "This repository has no commits yet, so there is nothing to copy. Commit once, or run it in this folder.",
        ));
    }

    let program = providers::which(provider.program()).ok_or_else(|| {
        AppError::new(
            ErrorKind::CliMissing,
            format!("{} is not installed or not on PATH.", provider.program()),
        )
    })?;
    let signals = routing::RepoSignals {
        tracked_paths: project::tracked_paths(&dir),
        recent_paths: project::recent_paths(&dir),
        prior_failures: store.prior_failures(project.id, &prompt)?,
        stalled_tiers: store.stalled_tiers(project.id, provider.program(), mode.name())?,
        // The frontend's reading, not a fresh one: the preview and the run
        // must be routed on the same number.
        headroom: headroom.filter(|room| room.is_finite()),
        ..Default::default()
    };
    Ok(Scanned {
        dir,
        project,
        program,
        git,
        prompt,
        mode,
        signals,
    })
}

/// Read the prompt and scan the repository at the same time, then route and
/// record the run. No agent starts before the route exists. The benchmark
/// calls this too, so it measures what the app does.
#[allow(clippy::too_many_arguments)]
async fn begin(
    store: &Store,
    recordings: Option<std::path::PathBuf>,
    path: String,
    prompt: String,
    provider: ProviderId,
    mode: Mode,
    headroom: Option<f64>,
    isolation: Isolation,
    continue_task: Option<i64>,
) -> Result<run::Request> {
    let started = std::time::Instant::now();
    let ms = |since: std::time::Instant| since.elapsed().as_millis() as u64;
    // Read on the provider the run will spend. Unreadable means the keyword
    // router decides, never a failed run. The same call names the task, so a
    // title costs no extra call; a prompt keywords already read gets none.
    let reading = match (run::clean_prompt(&prompt), providers::which(provider.program())) {
        (Some(text), Some(program)) if !routing::keywords_suffice(&text) => {
            Some(tokio::spawn(async move {
                let read = intent::read(provider, &program.to_string_lossy(), &text).await;
                (read, ms(started))
            }))
        }
        _ => None,
    };
    // Git, PATH and ACL probes, on this thread while the call runs elsewhere.
    // A map that fails to refresh only weakens the file list, never the run.
    let scanned = tokio::task::block_in_place(|| {
        if let Ok((dir, record)) = trusted_dir(store, &path) {
            let _ = store.scan_map(record.id, &dir, &project::tracked_paths(&dir));
        }
        scan_run(store, path, prompt, provider, mode, headroom, isolation)
    });
    let scan_ms = ms(started);
    let scanned = match scanned {
        Ok(scanned) => scanned,
        Err(e) => {
            // Dropping the call's process closes its job, which kills it.
            if let Some(reading) = reading {
                reading.abort();
            }
            return Err(e);
        }
    };
    let ((reading, classified), classify) = match reading {
        Some(handle) => match handle.await {
            Ok((read, took)) => (read, run::Timing { label: "classify".into(), ms: took }),
            Err(_) => (Default::default(), run::Timing { label: "classify".into(), ms: ms(started) }),
        },
        None => (Default::default(), run::Timing { label: "classify skipped".into(), ms: 0 }),
    };
    let planned = scanned.route(reading.intent, reading.job);
    let routed = std::time::Instant::now();
    let mut request = tokio::task::block_in_place(|| {
        prepare_run(store, recordings, planned, provider, mode, isolation, &reading.title, continue_task)
    })?;
    request.classified = classified;
    request.timings = vec![
        classify,
        run::Timing { label: "scan".into(), ms: scan_ms },
        run::Timing { label: "prepare".into(), ms: ms(routed) },
    ];
    Ok(request)
}

/// Show the exact route and ceilings before a provider is started.
#[tauri::command]
async fn preview_task(
    app: AppHandle,
    path: String,
    prompt: String,
    provider: ProviderId,
    mode: Mode,
    headroom: Option<f64>,
    isolation: Isolation,
) -> Result<Preflight> {
    // No classifier here: the preview refreshes as the user types, and a model
    // call per pause would cost more than the runs it routes. Off the main
    // thread: git, PATH and ACL probes froze the window on every pause.
    let planned = tauri::async_runtime::spawn_blocking(move || {
        scan_run(&app.state::<Store>(), path, prompt, provider, mode, headroom, isolation)
            .map(|scanned| scanned.route(None, None))
    })
    .await
    .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))??;
    let model = planned
        .route
        .work_model(provider)
        .unwrap_or_else(|| planned.route.budget.preferred_tier.model(provider));
    let plan = planned.route.plan_model(provider);
    let review = planned.route.review_model(provider);
    Ok(Preflight {
        provider,
        git: planned.git,
        route: planned.route,
        model,
        plan,
        review,
    })
}

/// How much of each plan's rolling limit is used, from each CLI's own answer.
/// Costs no tokens and never runs inside a project. Never an error: a provider
/// that cannot be read comes back with the reason.
#[tauri::command]
async fn provider_limits() -> Vec<providers::limits::Limits> {
    let probes = ProviderId::ALL.map(|id| tauri::async_runtime::spawn(providers::limits::read(id)));
    let mut limits = Vec::with_capacity(probes.len());
    for probe in probes {
        if let Ok(one) = probe.await {
            limits.push(one);
        }
    }
    limits
}

/// Everything that has to be true, and decided, before a provider starts: the
/// project is trusted, the CLI exists, the baseline is taken, and the route and
/// its ceilings are chosen and written down.
#[allow(clippy::too_many_arguments)]
fn prepare_run(
    store: &Store,
    recordings: Option<std::path::PathBuf>,
    planned: PlannedRun,
    provider: ProviderId,
    mode: Mode,
    isolation: Isolation,
    title: &str,
    continue_task: Option<i64>,
) -> Result<run::Request> {
    let PlannedRun {
        dir,
        project: record,
        program,
        git,
        prompt,
        route,
    } = planned;

    // A copy starts clean from HEAD: the user's uncommitted changes are not in it.
    let dirty_at_start = git.dirty && isolation == Isolation::CurrentTree;
    // The baseline is taken before the agent runs, so the diff afterwards has
    // something honest to compare against.
    // A clean tree needs no snapshot: everything in the diff is the run's.
    let before_run = if dirty_at_start {
        project::snapshot(&dir, git.head.as_deref())
    } else {
        Some(Default::default())
    };

    // Stored before anything runs, so a run that dies in its first second still
    // says what it was allowed to do.
    let route_json = serde_json::to_string(&route).ok();

    // A reply goes on in its own task, unless it runs in a copy: that folder
    // is not where the task's work is.
    let continued = continue_task.filter(|_| isolation == Isolation::CurrentTree);
    let task_id = match continued {
        Some(task_id) => {
            store.reopen_task(record.id, task_id, route_json.as_deref())?;
            task_id
        }
        None => store.create_task(NewTask {
            project_id: record.id,
            prompt: &prompt,
            title,
            mode: mode.name(),
            route_json: route_json.as_deref(),
            branch: git.branch.as_deref(),
            base_commit: git.head.as_deref(),
            dirty_at_start,
        })?,
    };

    let worktree = match isolation {
        Isolation::CurrentTree => None,
        Isolation::Worktree => match make_worktree(store, &dir, &git, provider, task_id) {
            Ok(copy) => Some(copy),
            Err(e) => {
                // The row is already open; it closes as what happened rather
                // than as a run left running.
                let _ = store
                    .finish_task_details(task_id, "failed", &e.message, "[]", None, 0, None, 0);
                return Err(e);
            }
        },
    };

    Ok(run::Request {
        task_id,
        id: provider,
        program,
        dir: worktree
            .as_ref()
            .map_or(dir, |copy| copy.path.clone().into()),
        prompt,
        route,
        base_commit: git.head,
        dirty_at_start,
        before_run,
        // A real run costs the user's subscription. Keeping its JSONL is what
        // makes the next one free, and is the only honest source of fixtures.
        recordings,
        worktree,
        classified: Vec::new(),
        attachments: Vec::new(),
        resume: None,
        continued: continued.is_some(),
        timings: Vec::new(),
        checking: None,
    })
}

/// Writes a pasted image where a run can read it, and returns its path. The
/// clipboard has no file behind it, so there is nothing else to attach.
#[tauri::command]
fn save_pasted_image(app: AppHandle, bytes: Vec<u8>, extension: String) -> Result<String> {
    let io = |e: std::io::Error| AppError::new(ErrorKind::Io, e.to_string());
    if !["png", "jpg", "jpeg", "gif", "webp"].contains(&extension.as_str()) {
        return Err(AppError::new(ErrorKind::Invalid, "Only images can be pasted."));
    }
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?
        .join("pasted");
    std::fs::create_dir_all(&dir).map_err(io)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = dir.join(format!("pasted-{stamp}.{extension}"));
    std::fs::write(&path, bytes).map_err(io)?;
    Ok(path.display().to_string())
}

/// Make the copy a run works in, on a branch named for its task.
fn make_worktree(
    store: &Store,
    repo: &std::path::Path,
    git: &GitState,
    provider: ProviderId,
    task_id: i64,
) -> Result<project::Worktree> {
    let copy = git.root.as_deref().and_then(|root| project::worktree_dir(root, task_id)).ok_or_else(|| {
        AppError::new(ErrorKind::Invalid, "A repository at the top of a drive has no folder beside it for a copy. Run it in this folder instead.")
    })?;
    let branch = format!("orteca/task-{task_id}");
    project::add_worktree(repo, &copy, &branch)?;
    let worktree = project::Worktree {
        path: copy.to_string_lossy().into_owned(),
        branch,
        commit: None,
        commit_error: None,
    };
    // Recorded before anything else can fail, so the copy can always be removed.
    store.set_worktree(task_id, &worktree.branch, &worktree.path)?;
    if provider == ProviderId::Codex && !proc::can_change_acl(&copy) {
        return Err(codex_acl_refusal());
    }
    Ok(worktree)
}

/// Delete a finished run's copy. Git refuses while it holds uncommitted work,
/// and the branch stays either way.
#[tauri::command]
fn remove_worktree(path: String, task_id: i64, store: State<Store>) -> Result<()> {
    let (dir, record) = trusted_dir(&store, &path)?;
    let Some(copy) = store.worktree_path(record.id, task_id)? else {
        return Err(AppError::new(
            ErrorKind::NotFound,
            "That run has no copy to remove.",
        ));
    };
    project::remove_worktree(&dir, std::path::Path::new(&copy))?;
    store.clear_worktree(record.id, task_id)
}

/// Open a file a run read or edited, or show it in Explorer. Only files in the
/// project or its run copies, and never a program: opening one would run it.
#[tauri::command]
fn open_file(path: String, file: String, reveal: bool, store: State<Store>) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let (dir, _) = trusted_dir(&store, &path)?;
    let target = dir
        .join(&file)
        .canonicalize()
        .ok()
        .filter(|t| t.is_file())
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, format!("{file} is not there anymore.")))?;
    let copies = dir.parent().map(|p| p.join(".orteca-worktrees"));
    if !(target.starts_with(&dir) || copies.is_some_and(|c| target.starts_with(c))) {
        return Err(AppError::new(ErrorKind::Invalid, "That file is outside this project."));
    }
    if !reveal && is_program(&target) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "That file is a program, so Orteca won't open it. Use Show in folder.",
        ));
    }
    // Explorer does not understand the `\\?\` form canonicalize returns.
    let shown = target.to_string_lossy();
    let shown = shown.strip_prefix(r"\\?\").unwrap_or(&shown);
    let mut explorer = std::process::Command::new("explorer");
    if reveal {
        explorer.raw_arg(format!("/select,\"{shown}\""));
    } else {
        explorer.arg(shown);
    }
    // Explorer's exit code means nothing; a spawn failure is the only real error.
    explorer.spawn()?;
    Ok(())
}

/// Anything Windows would run rather than open: PATHEXT plus the usual extras.
fn is_program(file: &std::path::Path) -> bool {
    let Some(ext) = file.extension().map(|e| format!(".{}", e.to_string_lossy()).to_ascii_uppercase()) else {
        return false;
    };
    let pathext = std::env::var("PATHEXT").unwrap_or_default().to_ascii_uppercase();
    pathext.split(';').any(|p| p.trim() == ext)
        || [
            ".EXE", ".COM", ".BAT", ".CMD", ".PS1", ".VBS", ".VBE", ".JS", ".JSE", ".WSF", ".WSH",
            ".MSI", ".MSC", ".CPL", ".SCR", ".HTA", ".LNK", ".URL", ".REG", ".PIF", ".APPREF-MS",
        ]
        .contains(&ext.as_str())
}

/// Fetch, pull, commit, push or merge, after the user confirmed it. Trusted projects
/// only: a commit runs the repository's own hooks. Returns the state after.
#[tauri::command]
async fn git_action(
    path: String,
    action: project::GitAction,
    input: String,
    store: State<'_, Store>,
) -> Result<GitState> {
    let (dir, _) = trusted_dir(&store, &path)?;
    tauri::async_runtime::spawn_blocking(move || {
        project::git_action(&dir, action, &input)?;
        Ok(project::git_state(&dir))
    })
    .await
    .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?
}

fn trusted_dir(store: &Store, path: &str) -> Result<(std::path::PathBuf, Project)> {
    let dir = project::validate_dir(path)?;
    let record = store.project(&dir.to_string_lossy())?;
    if !record.trusted {
        return Err(AppError::new(
            ErrorKind::NotTrusted,
            "This project has not been trusted yet.",
        ));
    }
    Ok((dir, record))
}

/// Install a provider CLI with npm, streaming its output as `install-event`.
/// Orteca never fetches a binary itself - it runs the package manager the user
/// already has, and a failed install is reported rather than left as a button
/// that appears to do nothing.
#[tauri::command(async)]
async fn install_provider(
    app: AppHandle,
    provider: ProviderId,
    operations: State<'_, ProviderOperations>,
) -> Result<Detected> {
    let npm = providers::which("npm").ok_or_else(|| {
        AppError::new(
            ErrorKind::CliMissing,
            "npm is not on PATH. Install Node.js first, then try again.",
        )
    })?;
    let cancel = operations.begin(provider)?;
    let install = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        npm_install(
            provider,
            &npm,
            |line| {
                let _ = app.emit("install-event", (provider, line));
            },
            cancel,
        ),
    )
    .await;
    operations.finish(provider);
    match install {
        Ok(result) => result?,
        Err(_) => {
            return Err(AppError::new(
                ErrorKind::Io,
                format!(
                    "{} installation timed out after 5 minutes.",
                    provider.program()
                ),
            ));
        }
    }

    let detected = tauri::async_runtime::spawn_blocking(move || provider.detect())
        .await
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
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<(Option<i32>, Vec<String>)> {
    let mut run = proc::spawn(&program.to_string_lossy(), args, &std::env::temp_dir())?;
    // Closed on purpose: nothing here can answer a prompt, so a CLI that wants
    // one must fail fast instead of hanging a button forever.
    run.close_stdin();

    let mut tail: Vec<String> = Vec::new();
    loop {
        let line = tokio::select! {
            line = run.lines.recv() => line,
            changed = cancel.changed() => {
                if changed.is_ok() && *cancel.borrow() {
                    run.cancel();
                    return Err(AppError::new(ErrorKind::Invalid, "Provider operation cancelled."));
                }
                continue;
            }
        };
        let Some(line) = line else { break };
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
        format!(
            "{} stopped without reporting an exit code",
            program.display()
        ),
    ))
}

async fn npm_install(
    provider: ProviderId,
    npm: &std::path::Path,
    emit: impl Fn(String),
    cancel: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    static INSTALL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _install = INSTALL.try_lock().map_err(|_| {
        AppError::new(
            ErrorKind::Invalid,
            "Another provider installation is already running.",
        )
    })?;
    let package = provider.package();
    let (code, tail) = stream_tool(npm, &["install", "--global", package], emit, cancel).await?;
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
async fn sign_in_provider(
    app: AppHandle,
    provider: ProviderId,
    operations: State<'_, ProviderOperations>,
) -> Result<Detected> {
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
    let cancel = operations.begin(provider)?;

    // A browser round trip is slow but not unbounded. Without this the button
    // would hang forever on a login the user abandoned - cancel arrives in M5.
    let stream = stream_tool(
        &program,
        provider.login_args(),
        |line| {
            let _ = app.emit("sign-in-event", (provider, line));
        },
        cancel,
    );
    let streamed = tokio::time::timeout(std::time::Duration::from_secs(300), stream).await;
    operations.finish(provider);
    let (code, tail) = match streamed {
        Ok(result) => result?,
        Err(_) => {
            return Err(AppError::new(
                ErrorKind::Io,
                format!("{} sign-in timed out after 5 minutes.", provider.program()),
            ));
        }
    };

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
            format!(
                "{} is still signed out (exit {code:?}).",
                provider.program()
            )
        } else {
            format!(
                "{} is still signed out: {}",
                provider.program(),
                tail.join(" ")
            )
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

/// Past runs in this project, newest first.
#[tauri::command]
fn recent_tasks(path: String, store: State<Store>) -> Result<Vec<store::TaskSummary>> {
    let dir = project::validate_dir(&path)?;
    store.recent_tasks(store.project(&dir.to_string_lossy())?.id, 20)
}

#[tauri::command]
fn rename_task(path: String, task_id: i64, title: String, store: State<Store>) -> Result<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "Type a name first."));
    }
    let title: String = title.chars().take(intent::TITLE_CHARS).collect();
    let dir = project::validate_dir(&path)?;
    store.rename_task(store.project(&dir.to_string_lossy())?.id, task_id, title.trim_end())
}

#[tauri::command]
fn delete_task(path: String, task_id: i64, store: State<Store>) -> Result<()> {
    let dir = project::validate_dir(&path)?;
    store.delete_task(store.project(&dir.to_string_lossy())?.id, task_id)
}

#[tauri::command]
fn task_detail(path: String, task_id: i64, store: State<Store>) -> Result<store::TaskDetail> {
    let dir = project::validate_dir(&path)?;
    store.task_detail(store.project(&dir.to_string_lossy())?.id, task_id)
}

#[tauri::command]
fn cancel_provider_operation(
    provider: ProviderId,
    operations: State<ProviderOperations>,
) -> Result<()> {
    operations.cancel(provider)
}

fn main() {
    tauri::Builder::default()
        // First, so a second launch exits before setup tries the database the
        // running instance holds, and the user sees their window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let db = app.path().app_data_dir()?.join("orteca.db");
            app.manage(Store::open(&db).map_err(|e| e.message)?);
            app.manage(run::Live::default());
            app.manage(ProviderOperations::default());
            // Codex runs are priced from this list. Offline keeps the last one.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Some(prices) = providers::codex::fetch_prices() {
                    let _ = handle.state::<Store>().save_prices(&prices);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_project,
            detect_providers,
            install_provider,
            sign_in_provider,
            recent_projects,
            start_task,
            save_pasted_image,
            cancel_task,
            send_instruction,
            trust_project,
            forget_project,
            recent_tasks,
            rename_task,
            delete_task,
            task_detail,
            preview_task,
            provider_limits,
            remove_worktree,
            open_file,
            git_action,
            cancel_provider_operation
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Orteca");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_are_not_opened() {
        assert!(is_program(std::path::Path::new(r"C:\x\setup.EXE")));
        assert!(is_program(std::path::Path::new("run.ps1")));
        assert!(!is_program(std::path::Path::new("app/Models/User.php")));
        assert!(!is_program(std::path::Path::new("Makefile")));
    }

    #[tokio::test]
    async fn concurrent_install_requests_are_rejected_on_the_backend() {
        let root = std::env::temp_dir().join(format!("orteca-install-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let fake = root.join("fake.cmd");
        std::fs::write(
            &fake,
            "@echo off\r\necho fake installer\r\nping -n 2 127.0.0.1 >nul\r\nexit /b 0\r\n",
        )
        .unwrap();
        let (first, second) = tokio::join!(
            npm_install(
                ProviderId::Codex,
                &fake,
                |_| {},
                tokio::sync::watch::channel(false).1
            ),
            npm_install(
                ProviderId::Claude,
                &fake,
                |_| {},
                tokio::sync::watch::channel(false).1
            ),
        );
        assert!(first.is_ok());
        assert!(second.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn provider_operations_can_be_cancelled_and_are_released() {
        let operations = ProviderOperations::default();
        let mut cancelled = operations.begin(ProviderId::Codex).unwrap();
        operations.cancel(ProviderId::Codex).unwrap();
        cancelled.changed().await.unwrap();
        assert!(*cancelled.borrow());
        operations.finish(ProviderId::Codex);
        assert!(operations.cancel(ProviderId::Codex).is_err());
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
        for path in [
            root.clone(),
            root.join("."),
            root.join("sub/.."),
            std::path::PathBuf::from(root.to_string_lossy().to_uppercase()),
        ] {
            let (dir, _) = trusted_dir(&store, path.to_str().unwrap()).unwrap();
            assert_eq!(dir, canonical);
        }
        assert!(trusted_dir(&store, root.join("sub").to_str().unwrap()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Spends real usage. One Orteca run for a benchmark driver:
    /// BENCH_DIR, BENCH_PROMPT, BENCH_PROVIDER (claude|codex), BENCH_MODE
    /// (efficient|balanced), BENCH_OUT (TaskResult JSON).
    /// `cargo test bench_run -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn bench_run() {
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k} not set"));
        let dir = std::path::PathBuf::from(var("BENCH_DIR")).canonicalize().unwrap();
        let key = dir.to_str().unwrap().to_string();
        let store = Store::in_memory().unwrap();
        store.touch_project(&key, "bench").unwrap();
        store.set_trusted(&key, true).unwrap();
        // The app loads this at startup; without it every Codex run is unpriced.
        if let Some(prices) = providers::codex::fetch_prices() {
            store.save_prices(&prices).unwrap();
        }
        // Started after the price fetch, which the app does once at startup.
        let t0 = std::time::Instant::now();
        fn json<T: serde::de::DeserializeOwned>(s: String) -> T {
            serde_json::from_value(serde_json::Value::String(s)).unwrap()
        }
        let request = begin(
            &store,
            None,
            key,
            var("BENCH_PROMPT"),
            json(var("BENCH_PROVIDER")),
            json(var("BENCH_MODE")),
            None,
            Isolation::CurrentTree,
            None,
        )
        .await
        .unwrap();
        let begun = t0.elapsed().as_millis() as u64;
        let result = run::stream(&store, &run::Live::default(), request, |_| Ok(())).await;
        // Wall time from the prompt, classify and scan included, which is what
        // a plain CLI's time is compared with.
        let mut out = serde_json::to_value(&result).unwrap();
        out["benchBeginMs"] = begun.into();
        out["benchWallMs"] = (t0.elapsed().as_millis() as u64).into();
        std::fs::write(var("BENCH_OUT"), serde_json::to_string_pretty(&out).unwrap()).unwrap();
    }

    /// Free: prints both CLIs' rolling limits as JSON.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn bench_limits() {
        for id in ProviderId::ALL {
            println!("LIMITS {}", serde_json::to_string(&providers::limits::read(id).await).unwrap());
        }
    }
}
