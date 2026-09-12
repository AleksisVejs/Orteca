#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod error;
mod project;
// Wired to provider invocation in Milestone 4.
#[allow(dead_code)]
mod proc;
// Detection is live. Event normalisation and the mock are exercised by tests
// until Milestone 4 runs a provider for real.
#[allow(dead_code)]
mod providers;
mod store;

use serde::Serialize;
use tauri::{Manager, State};

use error::{AppError, ErrorKind, Result};
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
    let root = git.root.clone().unwrap_or(path);
    let root_path = std::path::PathBuf::from(&root);

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
#[tauri::command(async)]
fn detect_providers() -> Vec<Detected> {
    ProviderId::ALL.into_iter().map(ProviderId::detect).collect()
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
            recent_projects,
            trust_project,
            forget_project
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Orteca");
}
