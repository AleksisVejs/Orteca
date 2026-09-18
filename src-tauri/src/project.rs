//! Opening a project: git state and the trust scan.

use std::path::{Path, PathBuf};
use std::os::windows::process::CommandExt;
use std::process::Command;

/// Orteca has no console, so every child would otherwise get its own window.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, ErrorKind, Result};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitState {
    pub is_repo: bool,
    pub root: Option<String>,
    /// Branch name, or `None` when HEAD is detached.
    pub branch: Option<String>,
    /// `None` on a repo with no commits yet.
    pub head: Option<String>,
    pub dirty: bool,
    pub dirty_count: usize,
    /// The remote branch this one tracks, and how far apart they were at the
    /// last fetch. All `None` when the branch tracks nothing.
    pub upstream: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    /// Local branches other than the current one: what Merge can bring in.
    pub branches: Vec<String>,
}

/// A git command the user picked by name and confirmed. None of them can
/// throw work away: pull only fast-forwards and push is never forced.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitAction {
    Fetch,
    Pull,
    Commit,
    Push,
    Merge,
}

/// A potential agent configuration source, or a path the scan could not inspect.
/// An empty list cannot establish trust: providers support custom filenames.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustFinding {
    pub path: String,
    pub reason: &'static str,
}

const TRUST_TARGETS: &[(&str, &str)] = &[
    (
        ".claude",
        "can contain settings, hooks, rules, skills, commands and subagents",
    ),
    (".mcp.json", "starts MCP servers as local processes"),
    ("CLAUDE.md", "injected into the agent's instructions"),
    ("CLAUDE.local.md", "injected into the agent's instructions"),
    ("AGENTS.md", "injected into the agent's instructions"),
    (
        "AGENTS.override.md",
        "overrides the agent's project instructions",
    ),
    (
        ".codex",
        "can contain Codex configuration and agent resources",
    ),
    (".agents", "can contain skills loaded by Codex"),
];

pub fn trust_scan(root: &Path) -> Vec<TrustFinding> {
    let mut findings = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    // Ancestor configuration can affect a session launched at the repository root.
    for parent in root.ancestors().skip(1) {
        scan_targets(root, parent, &mut findings);
    }
    let mut remaining = 10_000;
    while let Some(dir) = pending.pop() {
        scan_targets(root, &dir, &mut findings);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            findings.push(finding(
                root,
                &dir,
                "could not inspect this directory; configuration may be present",
            ));
            continue;
        };
        for entry in entries {
            if remaining == 0 {
                findings.push(finding(
                    root,
                    root,
                    "scan limit reached; additional configuration may be present",
                ));
                return findings;
            }
            remaining -= 1;
            let Ok(entry) = entry else {
                findings.push(finding(root, &dir, "could not inspect a directory entry"));
                continue;
            };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.eq_ignore_ascii_case(".git")
                || TRUST_TARGETS
                    .iter()
                    .any(|(target, _)| name.eq_ignore_ascii_case(target))
            {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                findings.push(finding(root, &path, "could not inspect this path"));
                continue;
            };
            use std::os::windows::fs::MetadataExt;
            // Junctions can escape the repository or cycle back into an ancestor.
            if metadata.file_attributes() & 0x400 != 0 {
                findings.push(finding(
                    root,
                    &path,
                    "linked path may load configuration outside the scanned tree",
                ));
            } else if metadata.is_dir() {
                pending.push(path);
            }
        }
    }
    findings
}

fn scan_targets(root: &Path, dir: &Path, findings: &mut Vec<TrustFinding>) {
    for (rel, reason) in TRUST_TARGETS {
        let path = dir.join(rel);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => findings.push(finding(root, &path, reason)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => findings.push(finding(
                root,
                &path,
                "could not inspect potential agent configuration",
            )),
        }
    }
}

fn finding(root: &Path, path: &Path, reason: &'static str) -> TrustFinding {
    let path = path.strip_prefix(root).unwrap_or(path);
    TrustFinding {
        path: if path.as_os_str().is_empty() {
            ".".into()
        } else {
            path.to_string_lossy().replace('\\', "/")
        },
        reason,
    }
}

pub fn git_state(dir: &Path) -> GitState {
    let Some(root) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return GitState::default();
    };

    let branch = git(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"]);

    let status = git(dir, &["status", "--porcelain"]).unwrap_or_default();
    let dirty_count = status.lines().filter(|l| !l.is_empty()).count();

    let upstream = git(dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]);
    let counts = upstream
        .as_ref()
        .and_then(|_| git(dir, &["rev-list", "--left-right", "--count", "HEAD...@{u}"]))
        .unwrap_or_default();
    let mut counts = counts.split_whitespace().map(|n| n.parse().ok());

    let branches = git(dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .unwrap_or_default()
        .lines()
        .filter(|b| Some(*b) != branch.as_deref())
        .map(String::from)
        .collect();

    GitState {
        branches,
        upstream,
        ahead: counts.next().flatten(),
        behind: counts.next().flatten(),
        is_repo: true,
        root: Some(root),
        branch,
        head: git(dir, &["rev-parse", "HEAD"]),
        dirty: dirty_count > 0,
        dirty_count,
    }
}

/// One changed file. Line counts are `None` for a binary file or an untracked
/// one, because git reports no numbers for either - and a zero would claim the
/// file was touched and nothing changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub path: String,
    pub added: Option<u64>,
    pub deleted: Option<u64>,
    /// Who made this change. `None` when there was no snapshot to compare
    /// against, and the screen must then say it cannot tell.
    pub origin: Option<Origin>,
}

/// Whether a changed file is this run's work, the user's, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// Clean when the run started.
    Run,
    /// Already changed when the run started, and byte-for-byte the same after.
    BeforeRun,
    /// Already changed when the run started, and different again after. The
    /// line counts are measured against the commit, so they include both.
    Both,
}

/// Every file that was already changed when a run started, with a fingerprint
/// of its contents. Taken before the first provider starts.
#[derive(Debug, Clone, Default, Hash)]
pub struct Snapshot(Vec<(String, Option<u64>)>);

/// Record what is already dirty, so the diff afterwards can tell the user's
/// changes from the agent's. `None` if git cannot list them: an unknown origin
/// is reported as unknown, never guessed as the run's.
pub fn snapshot(dir: &Path, base: Option<&str>) -> Option<Snapshot> {
    let dirty = diff_since(dir, base).ok()?;
    Some(Snapshot(
        dirty
            .into_iter()
            .map(|f| {
                let print = fingerprint(&dir.join(&f.path));
                (f.path, print)
            })
            .collect(),
    ))
}

/// Label each entry of a finished run's diff against the snapshot taken
/// before it. A file the run restored to the commit is no longer in the diff
/// at all, and so is not labelled.
pub fn attribute(dir: &Path, diff: &mut [FileStat], before: Option<&Snapshot>) {
    let Some(before) = before else { return };
    for file in diff {
        file.origin = Some(match before.0.iter().find(|(path, _)| *path == file.path) {
            None => Origin::Run,
            Some((_, print)) if *print == fingerprint(&dir.join(&file.path)) => Origin::BeforeRun,
            Some(_) => Origin::Both,
        });
    }
}

/// A content hash, streamed so a large file is never held in memory. `None`
/// for anything that is not a readable file - a deleted path stays `None`
/// until something recreates it. Only ever compared within one process.
fn fingerprint(path: &Path) -> Option<u64> {
    use std::hash::Hasher;
    struct Sink(std::collections::hash_map::DefaultHasher);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut file = std::fs::File::open(path)
        .ok()
        .filter(|f| f.metadata().is_ok_and(|m| m.is_file()))?;
    let mut sink = Sink(Default::default());
    std::io::copy(&mut file, &mut sink).ok()?;
    Some(sink.0.finish())
}

/// What the working tree looks like compared with `base`, plus whatever is
/// untracked. If the repo was already dirty when the run started, this mixes
/// the user's own edits in - the caller records `dirty_at_start` and says so.
pub fn diff_since(dir: &Path, base: Option<&str>) -> Result<Vec<FileStat>> {
    let mut stats = Vec::new();
    let empty_tree;
    let base = match base {
        Some(base) => base,
        None => {
            empty_tree = git_output(dir, &["hash-object", "-t", "tree", "--stdin"])?;
            empty_tree.trim()
        }
    };
    {
        let numstat = git_output(
            dir,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--numstat",
                "-z",
                base,
                "--",
            ],
        )?;
        let mut records = numstat.split('\0');
        while let Some(line) = records.next() {
            let mut parts = line.splitn(3, '\t');
            let (Some(added), Some(deleted), Some(path)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let path = if path.is_empty() {
                // -z emits both names separately when Git detects a rename.
                records.next();
                records.next().unwrap_or_default()
            } else {
                path
            };
            stats.push(FileStat {
                path: path.to_string(),
                added: added.parse().ok(),
                deleted: deleted.parse().ok(),
                origin: None,
            });
        }
    }
    let untracked = git_output(dir, &["ls-files", "-z", "--others", "--exclude-standard"])?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        stats.push(FileStat {
            path: path.to_string(),
            added: None,
            deleted: None,
            origin: None,
        });
    }
    Ok(stats)
}

/// The reviewable patch for the same comparison as `diff_since`.
///
/// Git does not include untracked files in `git diff`, so small text files are
/// rendered as new-file hunks here. Large and binary untracked files remain in
/// the file-stat list but are called out instead of loading unbounded data.
pub fn patch_since(dir: &Path, base: Option<&str>) -> Result<String> {
    const MAX_PATCH_BYTES: usize = 512 * 1024;
    const MAX_UNTRACKED_BYTES: usize = 64 * 1024;
    let empty_tree;
    let base = match base {
        Some(base) => base,
        None => {
            empty_tree = git_output(dir, &["hash-object", "-t", "tree", "--stdin"])?;
            empty_tree.trim()
        }
    };
    let mut patch = git_output(
        dir,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            "--full-index",
            base,
            "--",
        ],
    )?;
    let untracked = git_output(dir, &["ls-files", "-z", "--others", "--exclude-standard"])?;
    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
        if patch.len() >= MAX_PATCH_BYTES {
            break;
        }
        let full = dir.join(path);
        let remaining = MAX_PATCH_BYTES - patch.len();
        let addition = match std::fs::read(&full) {
            Ok(bytes) if bytes.len() <= MAX_UNTRACKED_BYTES => match String::from_utf8(bytes) {
                Ok(text) => new_file_patch(path, &text),
                Err(_) => format!("\n# binary untracked file: {path}\n"),
            },
            Ok(_) => format!("\n# untracked file too large to display: {path}\n"),
            Err(_) => format!("\n# untracked file could not be read: {path}\n"),
        };
        append_limited(&mut patch, &addition, remaining);
    }
    if patch.len() > MAX_PATCH_BYTES {
        truncate_utf8(&mut patch, MAX_PATCH_BYTES);
        patch.push_str("\n# patch truncated at 512 KiB\n");
    }
    Ok(patch)
}

fn append_limited(target: &mut String, text: &str, limit: usize) {
    let end = text
        .char_indices()
        .find(|(index, character)| *index + character.len_utf8() > limit)
        .map_or_else(|| limit.min(text.len()), |(index, _)| index);
    target.push_str(&text[..end]);
}

fn truncate_utf8(text: &mut String, limit: usize) {
    let mut end = limit.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

fn new_file_patch(path: &str, text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let count = lines.len();
    let mut patch = format!(
        "\ndiff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +{count} @@\n"
    );
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    patch
}

/// A test suite Orteca runs itself, with no model finding it.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    /// Ecosystem this command verifies. Used to skip unrelated suites after a
    /// run changes only one side of a mixed repository.
    pub kind: &'static str,
    /// Where it runs: the repository root or a folder directly under it.
    pub dir: PathBuf,
    /// Runs first, and only when the suite's dependencies are not installed:
    /// a fresh clone, or a run's separate copy, which git leaves without them.
    pub install: Option<Vec<&'static str>>,
    pub test: Vec<&'static str>,
}

/// Every test suite the repository declares, at its root and one folder down,
/// so a PHP + JS app is not verified by `npm test` alone and a `frontend/` or
/// `src-tauri/` suite is not missed. A root suite covers its own ecosystem, the
/// way a workspace's root `npm test` covers its packages.
pub fn check_commands(root: &Path) -> Vec<Check> {
    let mut checks = checks_in(root);
    let claimed: Vec<&str> = checks.iter().map(|(kind, _)| *kind).collect();
    let mut children: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        // `is_dir` on the entry's own type: a junction is not followed.
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            !name.starts_with('.')
                && !["node_modules", "vendor", "target", "dist", "build"].contains(&name.as_str())
        })
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        checks.extend(
            checks_in(&child)
                .into_iter()
                .filter(|(kind, _)| !claimed.contains(kind)),
        );
    }
    // ponytail: six suites bounds a monorepo's Verify; deeper layouts fall back
    // to the agent's Verify.
    checks.into_iter().map(|(_, check)| check).take(6).collect()
}

/// Keep only suites that can exercise the files this run changed. Unknown or
/// non-code-only changes keep the full set: skipping is allowed only when a
/// changed path positively identifies an ecosystem.
pub fn relevant_check_commands(root: &Path, changed_paths: &[String]) -> Vec<Check> {
    let checks = check_commands(root);
    let kinds: std::collections::HashSet<&str> = changed_paths
        .iter()
        .filter_map(|path| {
            let path = path.to_ascii_lowercase();
            let name = Path::new(&path).file_name()?.to_str()?;
            let extension = Path::new(&path).extension().and_then(|ext| ext.to_str());
            if name == "composer.json"
                || name == "composer.lock"
                || name.starts_with("phpunit.xml")
                || extension == Some("php")
            {
                Some("php")
            } else if name == "package.json"
                || [
                    "package-lock.json",
                    "pnpm-lock.yaml",
                    "yarn.lock",
                    "bun.lock",
                    "bun.lockb",
                ]
                .contains(&name)
                || ["js", "jsx", "ts", "tsx", "vue", "css", "scss"].contains(&extension?)
            {
                Some("js")
            } else if name == "cargo.toml" || name == "cargo.lock" || extension == Some("rs") {
                Some("rust")
            } else if name == "go.mod" || name == "go.sum" || extension == Some("go") {
                Some("go")
            } else if extension == Some("py")
                || ["pytest.ini", "pyproject.toml", "setup.cfg"].contains(&name)
            {
                Some("python")
            } else {
                None
            }
        })
        .collect();
    if kinds.is_empty() {
        return checks;
    }
    checks
        .into_iter()
        .filter(|check| kinds.contains(check.kind))
        .collect()
}

/// Build and lint commands the user explicitly requested, limited to scripts
/// the repository itself declares. They run as Orteca in a trusted project,
/// never as an arbitrary command invented by a model.
pub fn requested_check_commands(root: &Path, build: bool, lint: bool) -> Vec<Check> {
    if !build && !lint {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(scripts) = manifest["scripts"].as_object() else {
        return Vec::new();
    };
    let has_dependencies = ["dependencies", "devDependencies"]
        .iter()
        .any(|key| manifest[*key].as_object().is_some_and(|value| !value.is_empty()));
    let missing = has_dependencies && !root.join("node_modules").is_dir();
    let (program, install): (&'static str, Vec<&'static str>) =
        if root.join("pnpm-lock.yaml").is_file() {
            ("pnpm", vec!["pnpm", "install", "--frozen-lockfile"])
        } else if root.join("yarn.lock").is_file() {
            ("yarn", vec!["yarn", "install"])
        } else if root.join("bun.lock").is_file() || root.join("bun.lockb").is_file() {
            ("bun", vec!["bun", "install"])
        } else if root.join("package-lock.json").is_file() {
            ("npm", vec!["npm", "ci"])
        } else {
            ("npm", vec!["npm", "install", "--no-package-lock"])
        };
    let mut commands = Vec::new();
    for (asked, script) in [(build, "build"), (lint, "lint")] {
        if asked && scripts.get(script).and_then(|value| value.as_str()).is_some() {
            commands.push(Check {
                kind: "js",
                dir: root.to_path_buf(),
                install: missing.then(|| install.clone()),
                test: vec![program, "run", script],
            });
        }
    }
    commands
}

fn checks_in(dir: &Path) -> Vec<(&'static str, Check)> {
    let has = |file: &str| dir.join(file).exists();
    let manifest = |file: &str| {
        std::fs::read_to_string(dir.join(file))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
    };
    let mentions = |file: &str, needle: &str| {
        std::fs::read_to_string(dir.join(file)).is_ok_and(|text| text.contains(needle))
    };
    // `npm init`'s placeholder is not a test command.
    let tests = |m: &serde_json::Value| {
        let test = &m["scripts"]["test"];
        test.is_array() || test.as_str().is_some_and(|s| !s.contains("no test specified"))
    };
    // A manifest with nothing to install needs no install, which is also what
    // keeps a dependency-free suite from touching the network.
    let missing = |m: &serde_json::Value, keys: &[&str], folder: &str| {
        !has(folder) && keys.iter().any(|k| m[*k].as_object().is_some_and(|o| !o.is_empty()))
    };
    let check = |kind, install: Option<Vec<&'static str>>, test: Vec<&'static str>| Check {
        kind,
        dir: dir.to_path_buf(),
        install,
        test,
    };

    let mut out = Vec::new();
    if let Some(m) = manifest("package.json").filter(|m| tests(m)) {
        let (install, test) = if has("pnpm-lock.yaml") {
            (vec!["pnpm", "install", "--frozen-lockfile"], vec!["pnpm", "test"])
        } else if has("yarn.lock") {
            (vec!["yarn", "install"], vec!["yarn", "test"])
        } else if has("bun.lock") || has("bun.lockb") {
            (vec!["bun", "install"], vec!["bun", "run", "test"])
        } else if has("package-lock.json") {
            (vec!["npm", "ci"], vec!["npm", "test"])
        } else {
            // No lockfile, and Orteca does not add one to the user's diff.
            (vec!["npm", "install", "--no-package-lock"], vec!["npm", "test"])
        };
        let install = missing(&m, &["dependencies", "devDependencies"], "node_modules")
            .then_some(install);
        out.push(("js", check("js", install, test)));
    }
    if let Some(m) = manifest("composer.json") {
        let install = missing(&m, &["require", "require-dev"], "vendor")
            .then(|| vec!["composer", "install", "--no-interaction"]);
        if tests(&m) {
            out.push(("php", check("php", install, vec!["composer", "test"])));
        } else if has("phpunit.xml") || has("phpunit.xml.dist") {
            out.push((
                "php",
                check("php", install, vec!["php", "vendor/bin/phpunit"]),
            ));
        }
    }
    if has("Cargo.toml") {
        out.push(("rust", check("rust", None, vec!["cargo", "test"])));
    }
    if has("go.mod") {
        out.push(("go", check("go", None, vec!["go", "test", "./..."])));
    }
    // ponytail: the `python` on PATH, not the project's venv; pick the venv's
    // interpreter when a run shows that mattering.
    if has("pytest.ini")
        || mentions("pyproject.toml", "[tool.pytest")
        || mentions("setup.cfg", "[tool:pytest]")
        || mentions("tox.ini", "[pytest]")
    {
        out.push((
            "python",
            check("python", None, vec!["python", "-m", "pytest"]),
        ));
    }
    out
}

/// Every tracked path in the repository, as forward-slash relative paths.
///
/// One `git ls-files` and no file is opened: this is the cheap repository
/// signal the classifier scores a prompt's blast radius against. Capped,
/// because a monorepo should slow nothing down and a blast radius only has to
/// be big enough to leave the one-call route.
///
/// A repository git cannot list is not an error here. The classifier simply
/// sees no candidate paths and routes on the prompt alone.
pub fn tracked_paths(dir: &Path) -> Vec<String> {
    const CAP: usize = 20_000;
    git_output(dir, &["ls-files", "-z"])
        .unwrap_or_default()
        .split('\0')
        .filter(|p| !p.is_empty())
        .take(CAP)
        .map(str::to_string)
        .collect()
}

/// Paths touched by the last 50 commits, as `git log` names them. A ranking
/// signal only: a repository with no history simply has none.
pub fn recent_paths(dir: &Path) -> Vec<String> {
    git_output(
        dir,
        &[
            "-c",
            "core.quotepath=false",
            "log",
            "-n",
            "50",
            "--name-only",
            "--format=",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter(|p| !p.is_empty())
    // ponytail: one huge commit can list every file; the cap bounds it.
    .take(5_000)
    .map(str::to_string)
    .collect()
}

/// Display name for a project directory: the folder name.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub fn validate_dir(path: &str) -> Result<PathBuf> {
    let p = PathBuf::from(path);
    if !p.is_dir() {
        return Err(AppError::new(
            ErrorKind::NotFound,
            format!("{path} is not a folder that exists"),
        ));
    }
    Ok(p.canonicalize()?)
}

/// Where a run works: the user's own folder, or a copy of the last commit on a
/// branch of its own that the user merges when they are happy with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Isolation {
    #[default]
    CurrentTree,
    Worktree,
}

/// The separate copy a run worked in.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    /// The commit holding the run's changes. `None` when it changed nothing or
    /// git refused, which `commit_error` then says.
    pub commit: Option<String>,
    pub commit_error: Option<String>,
}

/// Beside the repository, not in app data: the folders above a copy are then
/// the ones the trust scan already read, plus one Orteca made. `None` for a
/// repository at the top of a drive, which has nowhere beside it.
pub fn worktree_dir(root: &str, task_id: i64) -> Option<PathBuf> {
    let root = PathBuf::from(root.replace('/', std::path::MAIN_SEPARATOR_STR));
    Some(
        root.parent()?
            .join(".orteca-worktrees")
            .join(format!("{}-{task_id}", display_name(&root))),
    )
}

/// A hooks folder that never exists, so git runs none of the repository's
/// hooks: `--no-verify` alone still runs post-checkout and post-commit.
fn no_hooks(copy: &Path) -> String {
    format!(
        "core.hooksPath={}",
        copy.parent().unwrap_or(copy).join(".no-hooks").display()
    )
}

pub fn add_worktree(repo: &Path, copy: &Path, branch: &str) -> Result<()> {
    if let Some(parent) = copy.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let hooks = no_hooks(copy);
    let path = copy.to_string_lossy();
    git_run(
        repo,
        &[
            "-c", &hooks, "worktree", "add", "-q", "-b", branch, &path, "HEAD",
        ],
        "Git could not make a separate copy",
    )?;
    Ok(())
}

/// Commit everything in a copy to its branch, as Orteca rather than the user:
/// the agent wrote it, and a missing identity or a signing prompt must not
/// leave the work uncommitted.
pub fn commit_worktree(copy: &Path, message: &str) -> Result<String> {
    let hooks = no_hooks(copy);
    let quiet = [
        "-c",
        &hooks,
        "-c",
        "commit.gpgsign=false",
        "-c",
        "user.name=Orteca",
        "-c",
        "user.email=orteca@localhost",
    ];
    git_run(
        copy,
        &[&quiet[..], &["add", "-A"]].concat(),
        "Git could not stage the run's changes",
    )?;
    git_run(
        copy,
        &[&quiet[..], &["commit", "-q", "--no-verify", "-m", message]].concat(),
        "Git could not commit the run's changes",
    )?;
    git(copy, &["rev-parse", "HEAD"])
        .ok_or_else(|| AppError::new(ErrorKind::Io, "Git committed but could not name the commit"))
}

/// Never forced: a copy with uncommitted work stays, and git says why. The
/// branch is left alone, because deleting it would be a destructive command.
pub fn remove_worktree(repo: &Path, copy: &Path) -> Result<()> {
    if !copy.exists() {
        // Already deleted by hand: only git's record of it is left.
        git_run(
            repo,
            &["worktree", "prune"],
            "Git could not forget the missing copy",
        )?;
        return Ok(());
    }
    git_run(
        repo,
        &["worktree", "remove", &copy.to_string_lossy()],
        "Git would not remove the copy",
    )?;
    Ok(())
}

/// A throwaway checkout of `base` for asking whether a test already failed
/// before a run. The user's tree is only read. `setup` names untracked paths a
/// suite needs (`vendor`, `.env`), copied in from `repo` when they exist: a
/// junction would load the changed tree's classes, not the base's.
// ponytail: `base` is the commit, so a run that started dirty is compared with
// HEAD, not with the user's edits; snapshot the dirty patch if that misleads.
pub fn base_copy(repo: &Path, base: &str, copy: &Path, setup: &[&str]) -> Result<()> {
    drop_base_copy(repo, copy);
    let hooks = no_hooks(copy);
    git_run(
        repo,
        &["-c", &hooks, "worktree", "add", "-q", "--detach", &copy.to_string_lossy(), base],
        "Git could not check out the base commit",
    )?;
    for path in setup {
        // robocopy takes no `\\?\` path, which is what a project folder canonicalizes to.
        let (from, to) = (plain(&repo.join(path)), plain(&copy.join(path)));
        if from.is_dir() {
            // robocopy exits 0-7 on success; 8 and up is a failure.
            let code = Command::new("robocopy")
                .arg(&from)
                .arg(&to)
                .args(["/E", "/MT:16", "/NFL", "/NDL", "/NJH", "/NJS", "/NP"])
                .creation_flags(CREATE_NO_WINDOW)
                .status()?
                .code();
            if !code.is_some_and(|c| c < 8) {
                return Err(AppError::new(ErrorKind::Io, format!("Could not copy {path}")));
            }
        } else if from.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// `path` without the `\\?\` prefix `canonicalize` gives on Windows.
pub fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
}

/// Deletes the folder itself, then has git forget it. Nothing in it is the
/// user's, and `worktree remove` refuses a copy the tests left files in.
pub fn drop_base_copy(repo: &Path, copy: &Path) {
    let _ = std::fs::remove_dir_all(copy);
    let _ = git_run(repo, &["worktree", "prune"], "");
}

/// Runs as the user, with their identity and their hooks: this is their
/// commit, not a run's. `input` is the commit message or the branch to merge.
pub fn git_action(dir: &Path, action: GitAction, input: &str) -> Result<()> {
    match action {
        GitAction::Fetch => git_run(dir, &["fetch"], "Git could not fetch"),
        GitAction::Pull => git_run(dir, &["pull", "--ff-only"], "Git could not pull"),
        GitAction::Commit => {
            let message = input.trim();
            if message.is_empty() {
                return Err(AppError::new(ErrorKind::Invalid, "A commit needs a message."));
            }
            git_run(dir, &["add", "-A"], "Git could not stage your changes")?;
            git_run(dir, &["commit", "-q", "-m", message], "Git could not commit")
        }
        // A branch that tracks nothing yet gets linked on its first push.
        GitAction::Push if git(dir, &["rev-parse", "--abbrev-ref", "@{u}"]).is_some() => {
            git_run(dir, &["push"], "Git could not push")
        }
        GitAction::Push => {
            let remotes = git(dir, &["remote"]).unwrap_or_default();
            let remote = remotes
                .lines()
                .find(|r| *r == "origin")
                .or_else(|| remotes.lines().next())
                .ok_or_else(|| AppError::new(ErrorKind::Invalid, "This repository has no remote to push to."))?;
            git_run(dir, &["push", "-u", remote, "HEAD"], "Git could not push")
        }
        GitAction::Merge => merge(dir, input),
    }
    .map(drop)
}

/// Only a clean tree, so a clash can be undone with nothing of the user's
/// caught in it: the merge either lands whole or leaves no trace.
fn merge(dir: &Path, branch: &str) -> Result<String> {
    if !git_state(dir).branches.iter().any(|b| b == branch) {
        return Err(AppError::new(ErrorKind::Invalid, "That branch does not exist here."));
    }
    if git(dir, &["status", "--porcelain"]).is_some() {
        return Err(AppError::new(ErrorKind::Invalid, "Commit your changes before merging."));
    }
    git_run(dir, &["merge", "--no-edit", branch], "Git could not merge").map_err(|e| {
        // A clash is reported on stdout, so the error text alone may be empty.
        let clashes = git(dir, &["diff", "--name-only", "--diff-filter=U"]);
        let _ = git_run(dir, &["merge", "--abort"], "");
        let reason = match clashes {
            Some(files) => format!("{branch} and this branch change the same lines in: {}.", files.lines().collect::<Vec<_>>().join(", ")),
            None => e.message,
        };
        AppError::new(ErrorKind::Invalid, format!("{reason} Nothing was changed."))
    })
}

/// A fresh machine may have no git at all, and every folder would then look
/// like "not a repository".
pub fn git_installed() -> bool {
    Command::new("git")
        .arg("--version")
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Run git and return trimmed stdout, or `None` if git failed or isn't there.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let text = git_output(dir, args).ok()?.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn git_output(dir: &Path, args: &[&str]) -> Result<String> {
    git_run(dir, args, "Git could not inspect changes")
}

fn git_run(dir: &Path, args: &[&str], failure: &str) -> Result<String> {
    // Even `git status` can execute a repository's core.fsmonitor command.
    let out = Command::new("git")
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
        .args(args)
        // A credential prompt on a terminal nobody sees would hang forever.
        .env("GIT_TERMINAL_PROMPT", "0")
        .creation_flags(CREATE_NO_WINDOW)
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(AppError::new(
            ErrorKind::Io,
            format!("{failure}: {}", String::from_utf8_lossy(&out.stderr).trim()),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mixed_repository_declares_every_suite() {
        let dir = temp_dir("check-commands");
        let found = |dir: &Path| {
            check_commands(dir)
                .into_iter()
                .map(|c| (c.dir, c.install, c.test))
                .collect::<Vec<_>>()
        };
        assert!(found(&dir).is_empty());
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        assert!(found(&dir).is_empty(), "npm init's placeholder");

        // A Laravel app with its tests at the root and a Vite front end beside it,
        // freshly cloned: nothing installed yet.
        std::fs::remove_file(dir.join("package.json")).unwrap();
        std::fs::write(
            dir.join("composer.json"),
            r#"{"require":{"laravel/framework":"^12"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("phpunit.xml"), "<phpunit/>").unwrap();
        let web = dir.join("frontend");
        std::fs::create_dir_all(&web).unwrap();
        std::fs::write(
            web.join("package.json"),
            r#"{"scripts":{"test":"vitest run"},"devDependencies":{"vite":"^6"}}"#,
        )
        .unwrap();
        std::fs::write(web.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(
            found(&dir),
            [
                (
                    dir.clone(),
                    Some(vec!["composer", "install", "--no-interaction"]),
                    vec!["php", "vendor/bin/phpunit"]
                ),
                (
                    web.clone(),
                    Some(vec!["pnpm", "install", "--frozen-lockfile"]),
                    vec!["pnpm", "test"]
                ),
            ]
        );

        // Installed, nothing is installed again; a root JS suite claims JS.
        std::fs::create_dir_all(dir.join("vendor")).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"scripts":{"test":"node --test"}}"#).unwrap();
        assert_eq!(
            found(&dir),
            [
                (dir.clone(), None, vec!["npm", "test"]),
                (dir.clone(), None, vec!["php", "vendor/bin/phpunit"]),
            ]
        );
        assert_eq!(
            relevant_check_commands(&dir, &["app/Console/Command.php".into()])
                .into_iter()
                .map(|check| check.kind)
                .collect::<Vec<_>>(),
            ["php"],
            "a backend-only change ran an unrelated JavaScript suite"
        );
        assert_eq!(
            relevant_check_commands(&dir, &["src/app.ts".into()])
                .into_iter()
                .map(|check| check.kind)
                .collect::<Vec<_>>(),
            ["js"],
            "a frontend-only change ran an unrelated PHP suite"
        );
        assert_eq!(
            relevant_check_commands(&dir, &["database/schema/mysql-schema.sql".into()]).len(),
            2,
            "an unknown-only change guessed which suite could cover it"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn requested_package_actions_use_only_declared_scripts() {
        let dir = temp_dir("requested-checks");
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"build":"vite build","lint":"eslint ."}}"#,
        )
        .unwrap();
        assert_eq!(
            requested_check_commands(&dir, true, true)
                .into_iter()
                .map(|check| check.test)
                .collect::<Vec<_>>(),
            [vec!["npm", "run", "build"], vec!["npm", "run", "lint"]]
        );
        assert!(requested_check_commands(&dir, false, false).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_commit_pushed_from_one_clone_is_fetched_and_pulled_by_another() {
        let root = temp_dir("git-actions");
        let run = |dir: &Path, args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        };
        run(&root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        for clone in ["mine", "theirs"] {
            run(&root, &["clone", "-q", "remote.git", clone]);
            let dir = root.join(clone);
            run(&dir, &["config", "user.name", "test"]);
            run(&dir, &["config", "user.email", "test@example.com"]);
            run(&dir, &["checkout", "-q", "-b", "main"]);
        }
        let (mine, theirs) = (root.join("mine"), root.join("theirs"));

        std::fs::write(mine.join("a.txt"), "one\n").unwrap();
        assert!(git_action(&mine, GitAction::Commit, "  ").is_err(), "an empty message was committed");
        git_action(&mine, GitAction::Commit, "Add a").unwrap();
        assert_eq!(git_state(&mine).upstream, None);
        git_action(&mine, GitAction::Push, "").unwrap();
        assert_eq!(git_state(&mine).upstream.as_deref(), Some("origin/main"), "first push did not link the branch");
        run(&theirs, &["fetch", "-q"]);
        run(&theirs, &["checkout", "-q", "-B", "main", "--track", "origin/main"]);

        std::fs::write(mine.join("a.txt"), "two\n").unwrap();
        git_action(&mine, GitAction::Commit, "Change a").unwrap();
        assert_eq!(git_state(&mine).ahead, Some(1));
        git_action(&mine, GitAction::Push, "").unwrap();
        assert_eq!(git_state(&mine).ahead, Some(0));

        assert_eq!(git_state(&theirs).behind, Some(0), "behind before any fetch");
        git_action(&theirs, GitAction::Fetch, "").unwrap();
        assert_eq!(git_state(&theirs).behind, Some(1));
        git_action(&theirs, GitAction::Pull, "").unwrap();
        let state = git_state(&theirs);
        assert_eq!((state.ahead, state.behind), (Some(0), Some(0)));
        assert_eq!(std::fs::read_to_string(theirs.join("a.txt")).unwrap().trim(), "two");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_merge_lands_whole_or_leaves_no_trace() {
        let dir = temp_dir("git-merge");
        let run = |args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(&dir).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "test@example.com"]);
        let commit = |file: &str, text: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            git_action(&dir, GitAction::Commit, file).unwrap();
        };
        commit("a.txt", "base\n");
        run(&["checkout", "-q", "-b", "clean"]);
        commit("b.txt", "new\n");
        run(&["checkout", "-q", "-b", "clash", "main"]);
        commit("a.txt", "theirs\n");
        run(&["checkout", "-q", "main"]);
        commit("a.txt", "ours\n");

        assert_eq!(git_state(&dir).branches, ["clash", "clean"]);
        assert!(git_action(&dir, GitAction::Merge, "--help").is_err());

        let head = git(&dir, &["rev-parse", "HEAD"]);
        let clash = git_action(&dir, GitAction::Merge, "clash").unwrap_err();
        assert!(clash.message.contains("a.txt") && clash.message.contains("Nothing was changed"), "{}", clash.message);
        assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
        assert!(!git_state(&dir).dirty, "a clash left the tree mid-merge");

        std::fs::write(dir.join("a.txt"), "unsaved\n").unwrap();
        assert!(git_action(&dir, GitAction::Merge, "clean").is_err(), "merged over uncommitted work");
        git_action(&dir, GitAction::Commit, "save").unwrap();

        git_action(&dir, GitAction::Merge, "clean").unwrap();
        assert!(dir.join("b.txt").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn diff_in_an_unborn_repo_includes_staged_files() {
        let dir = temp_dir("diff-unborn");
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success());
        std::fs::write(dir.join("staged.txt"), "one\n").unwrap();
        assert!(Command::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success());
        let stats = diff_since(&dir, None).unwrap();
        assert!(stats.iter().any(|s| s.path == "staged.txt"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn diff_preserves_unicode_spaces_and_rename_destinations() {
        let dir = temp_dir("diff-paths");
        let command = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        command(&["init", "-q"]);
        command(&["config", "user.name", "test"]);
        command(&["config", "user.email", "test@example.com"]);
        for name in ["old.txt", "é file.txt", "binary.dat"] {
            std::fs::write(
                dir.join(name),
                if name == "binary.dat" {
                    b"\0old"
                } else {
                    b"old\n"
                },
            )
            .unwrap();
        }
        command(&["add", "."]);
        command(&["commit", "-qm", "initial"]);
        let base = git_state(&dir).head.unwrap();
        command(&["mv", "old.txt", "new name.txt"]);
        std::fs::write(dir.join("é file.txt"), "old\nnew\n").unwrap();
        std::fs::write(dir.join("binary.dat"), b"\0new").unwrap();
        std::fs::write(dir.join("新 file.txt"), "untracked").unwrap();
        let stats = diff_since(&dir, Some(&base)).unwrap();
        for name in ["new name.txt", "é file.txt", "binary.dat", "新 file.txt"] {
            assert!(
                stats.iter().any(|f| f.path == name),
                "missing {name}: {stats:?}"
            );
        }
        let text = stats.iter().find(|f| f.path == "é file.txt").unwrap();
        assert_eq!((text.added, text.deleted), (Some(1), Some(0)));
        assert_eq!(
            stats.iter().find(|f| f.path == "binary.dat").unwrap().added,
            None
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn patch_contains_tracked_hunks_and_small_untracked_files() {
        let dir = temp_dir("patch");
        let command = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        command(&["init", "-q"]);
        command(&["config", "user.name", "test"]);
        command(&["config", "user.email", "test@example.com"]);
        std::fs::write(dir.join("tracked.txt"), "before\n").unwrap();
        command(&["add", "."]);
        command(&["commit", "-qm", "initial"]);
        let base = git_state(&dir).head.unwrap();
        std::fs::write(dir.join("tracked.txt"), "before\nafter\n").unwrap();
        std::fs::write(dir.join("new.txt"), "new content\n").unwrap();

        let patch = patch_since(&dir, Some(&base)).unwrap();
        assert!(patch.contains("+after"), "tracked hunk missing: {patch}");
        assert!(
            patch.contains("diff --git a/new.txt b/new.txt"),
            "untracked hunk missing: {patch}"
        );
        assert!(
            patch.contains("+new content"),
            "untracked content missing: {patch}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The sandbox case that prompted this: `stray.txt` was untracked before
    /// the run and must not be reported as the agent's work.
    #[test]
    fn a_diff_tells_the_users_changes_from_the_runs() {
        let dir = temp_dir("diff-origin");
        let command = |args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        command(&["init", "-q"]);
        command(&["config", "user.name", "test"]);
        command(&["config", "user.email", "test@example.com"]);
        for name in ["clean.txt", "edited.txt", "gone.txt"] {
            std::fs::write(dir.join(name), "old\n").unwrap();
        }
        command(&["add", "."]);
        command(&["commit", "-qm", "initial"]);
        let base = git_state(&dir).head.unwrap();

        // The user's own changes, before any run.
        std::fs::write(dir.join("stray.txt"), "untracked\n").unwrap();
        std::fs::write(dir.join("edited.txt"), "old\nmine\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        let before = snapshot(&dir, Some(&base)).expect("snapshot");

        // The run: touches a clean file and one the user had already edited.
        std::fs::write(dir.join("clean.txt"), "old\nagent\n").unwrap();
        std::fs::write(dir.join("edited.txt"), "old\nmine\nagent\n").unwrap();
        std::fs::write(dir.join("created.txt"), "agent\n").unwrap();

        let mut diff = diff_since(&dir, Some(&base)).unwrap();
        attribute(&dir, &mut diff, Some(&before));
        let origin = |name: &str| {
            diff.iter()
                .find(|f| f.path == name)
                .unwrap_or_else(|| panic!("missing {name}"))
                .origin
        };
        assert_eq!(
            origin("stray.txt"),
            Some(Origin::BeforeRun),
            "an untracked file from before the run was claimed"
        );
        assert_eq!(
            origin("gone.txt"),
            Some(Origin::BeforeRun),
            "a deletion from before the run was claimed"
        );
        assert_eq!(origin("edited.txt"), Some(Origin::Both));
        assert_eq!(origin("clean.txt"), Some(Origin::Run));
        assert_eq!(origin("created.txt"), Some(Origin::Run));

        // No snapshot means no claim either way.
        let mut unknown = diff_since(&dir, Some(&base)).unwrap();
        attribute(&dir, &mut unknown, None);
        assert!(unknown.iter().all(|f| f.origin.is_none()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A copy is its own folder on its own branch: the run's work is committed
    /// there, the user's folder and uncommitted edits stay as they were, no
    /// repository hook runs, and removing the copy keeps the branch.
    #[test]
    fn a_worktree_keeps_the_run_away_from_the_users_folder() {
        let base = temp_dir("worktree");
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git_in = |dir: &Path, args: &[&str]| {
            let out = Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git_in(&repo, &["init", "-q"]);
        git_in(&repo, &["config", "user.name", "test"]);
        git_in(&repo, &["config", "user.email", "test@example.com"]);
        std::fs::write(repo.join("a.txt"), "old\n").unwrap();
        git_in(&repo, &["add", "."]);
        git_in(&repo, &["commit", "-qm", "initial"]);
        for hook in ["post-checkout", "post-commit"] {
            std::fs::write(
                repo.join(".git/hooks").join(hook),
                "#!/bin/sh\ntouch hook-ran\n",
            )
            .unwrap();
        }
        std::fs::write(repo.join("a.txt"), "mine\n").unwrap();

        let copy = worktree_dir(&git_state(&repo).root.unwrap(), 7).unwrap();
        assert!(
            copy.ends_with(Path::new(".orteca-worktrees").join("repo-7")),
            "{}",
            copy.display()
        );
        add_worktree(&repo, &copy, "orteca/task-7").unwrap();
        assert_eq!(
            std::fs::read_to_string(copy.join("a.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "old\n",
            "the copy took the user's uncommitted edit"
        );

        std::fs::write(copy.join("a.txt"), "agent\n").unwrap();
        std::fs::write(copy.join("new.txt"), "agent\n").unwrap();
        let sha = commit_worktree(&copy, "Orteca task 7: test").unwrap();
        assert_eq!(git_in(&repo, &["rev-parse", "orteca/task-7"]), sha);
        assert_eq!(
            git_in(&repo, &["log", "-1", "--format=%an", "orteca/task-7"]),
            "Orteca"
        );
        assert!(
            !copy.join("hook-ran").exists() && !repo.join("hook-ran").exists(),
            "a repository hook ran"
        );
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap(),
            "mine\n",
            "the user's folder changed"
        );
        assert!(!repo.join("new.txt").exists());

        remove_worktree(&repo, &copy).unwrap();
        assert!(!copy.exists());
        git_in(&repo, &["rev-parse", "--verify", "orteca/task-7"]);
        std::fs::remove_dir_all(base).unwrap();
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("orteca-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn trust_scan_covers_nested_and_ancestor_instruction_sources() {
        let parent = temp_dir("trust-scopes");
        let root = parent.join("repo");
        let nested = root.join("packages/ui");
        std::fs::create_dir_all(&nested).unwrap();
        let files = [
            parent.join("CLAUDE.local.md"),
            root.join("AGENTS.override.md"),
            root.join(".claude/CLAUDE.md"),
            nested.join("CLAUDE.md"),
            nested.join(".claude/rules/test.md"),
            nested.join(".claude/skills/test/SKILL.md"),
            nested.join(".agents/skills/test/SKILL.md"),
        ];
        for file in &files {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "instructions").unwrap();
        }
        let findings = trust_scan(&root);
        for file in &files {
            assert!(
                findings.iter().any(|finding| {
                    let source = root.join(&finding.path);
                    file == &source || file.starts_with(&source)
                }),
                "missed {}",
                file.display()
            );
        }
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn trust_scan_reports_junctions_without_following_cycles() {
        let dir = temp_dir("trust-junction");
        let link = dir.join("cycle");
        let output = Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let findings = trust_scan(&dir);
        std::fs::remove_dir(&link).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(findings
            .iter()
            .any(|f| f.path == "cycle" && f.reason.contains("linked path")));
        assert!(!findings.iter().any(|f| f.reason.contains("scan limit")));
    }

    #[test]
    fn trust_scan_reports_unreadable_roots() {
        let dir = temp_dir("trust-unreadable");
        let file = dir.join("not-a-directory");
        std::fs::write(&file, "test").unwrap();
        assert!(trust_scan(&file)
            .iter()
            .any(|f| f.reason.contains("could not inspect this directory")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn trust_scan_finds_executable_config() {
        let dir = temp_dir("trust");
        let inherited: Vec<_> = trust_scan(&dir).into_iter().map(|f| f.path).collect();

        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(dir.join("CLAUDE.md"), "# hi").unwrap();

        let found: Vec<_> = trust_scan(&dir)
            .into_iter()
            .map(|f| f.path)
            .filter(|path| !inherited.contains(path))
            .collect();
        assert!(found.contains(&".claude".to_string()));
        assert!(found.contains(&"CLAUDE.md".to_string()));
        assert_eq!(found.len(), 2, "only what exists is reported");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn git_state_reports_not_a_repo_for_a_plain_folder() {
        let dir = temp_dir("nogit");
        let state = git_state(&dir);
        assert!(!state.is_repo);
        assert!(state.head.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn git_state_sees_branch_and_dirtiness() {
        let dir = temp_dir("git");
        let run = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);

        // No commits yet: a repo, but no HEAD.
        assert!(git_state(&dir).is_repo);
        assert!(git_state(&dir).head.is_none());
        assert_eq!(git_state(&dir).branch.as_deref(), Some("main"));

        std::fs::write(dir.join("a.txt"), "one").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);

        let clean = git_state(&dir);
        assert_eq!(clean.branch.as_deref(), Some("main"));
        assert!(clean.head.is_some());
        assert!(!clean.dirty, "just committed, should be clean");

        std::fs::write(dir.join("a.txt"), "two").unwrap();
        let dirty = git_state(&dir);
        assert!(dirty.dirty);
        assert_eq!(dirty.dirty_count, 1);

        run(&["checkout", "--detach", "-q"]);
        assert!(git_state(&dir).branch.is_none());
        assert!(git_state(&dir).head.is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn root_identity_survives_subdirectories_and_separators() {
        let dir = temp_dir("spaces-\u{e9}");
        let dir = dir.join("repo with spaces");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        assert!(Command::new("git")
            .args(["init", "-q"])
            .current_dir(&dir)
            .status()
            .unwrap()
            .success());
        let store = crate::store::Store::in_memory().unwrap();
        let root = git_state(&dir).root.unwrap();
        for path in [
            dir.clone(),
            dir.join("sub"),
            PathBuf::from(format!("{}\\", dir.display())),
            PathBuf::from(&root),
            PathBuf::from(dir.to_string_lossy().to_uppercase()),
        ] {
            let found = git_state(&path).root.unwrap();
            assert_eq!(found, root);
            store.touch_project(&found, "repo").unwrap();
        }
        assert_eq!(store.recent_projects(10).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_paths_return_errors() {
        let dir = temp_dir("invalid");
        let file = dir.join("file");
        std::fs::write(&file, "text").unwrap();
        assert!(validate_dir(file.to_str().unwrap()).is_err());
        assert!(validate_dir(dir.join("missing").to_str().unwrap()).is_err());
        assert!(validate_dir("").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_git_does_not_panic() {
        if std::env::var_os("ORTECA_TEST_NO_GIT").is_some() {
            assert!(!git_state(&std::env::temp_dir()).is_repo);
            return;
        }
        let result = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "project::tests::missing_git_does_not_panic"])
            .env("ORTECA_TEST_NO_GIT", "1")
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    #[test]
    fn git_status_does_not_execute_repository_fsmonitor() {
        let dir = temp_dir("fsmonitor");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("tracked"), "test").unwrap();
        git(&["add", "tracked"]);
        git(&["config", "core.fsmonitor", "echo executed > fsmonitor-ran"]);
        assert!(git_state(&dir).is_repo);
        assert!(
            !dir.join("fsmonitor-ran").exists(),
            "opening a repository executed its fsmonitor command before consent"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
