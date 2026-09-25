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
    /// Every file Git reports as changed, with its porcelain status code.
    pub changes: Vec<GitChange>,
    /// The remote branch this one tracks, and how far apart they were at the
    /// last fetch. All `None` when the branch tracks nothing.
    pub upstream: Option<String>,
    pub ahead: Option<usize>,
    pub behind: Option<usize>,
    /// Local branches other than the current one: what Merge can bring in.
    pub branches: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    pub path: String,
    /// The two-character `git status --porcelain` code, e.g. ` M` or `??`.
    pub status: String,
}

/// A git command the user picked by name and confirmed.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GitAction {
    Fetch,
    Pull,
    Commit,
    Push,
    Merge,
    Discard,
    Switch,
    Branch,
}

/// A potential agent configuration source, or a path the scan could not inspect.
/// An empty list cannot establish trust: providers support custom filenames.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustFinding {
    pub path: String,
    pub reason: &'static str,
}

/// Folders a build regenerates. No test suite is looked for in one, and the
/// trust scan walks none of them: a single `target/` is tens of thousands of
/// entries, enough to spend the scan's whole budget before it reaches the
/// repository's own source. Their top level is still scanned.
const GENERATED_DIRS: &[&str] = &["node_modules", "vendor", "target", "dist", "build"];
const GENERATED: &str = "generated directory: only its top level was scanned, \
                         and configuration nested deeper was not checked";

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
                if GENERATED_DIRS.iter().any(|d| name.eq_ignore_ascii_case(d)) {
                    scan_targets(root, &path, &mut findings);
                    // Skipped, and said so. Walking one `target/` can spend the
                    // whole budget before the scan reaches the repository's own
                    // source, but a skip the user is not told about is worse
                    // than the cost: the consent dialog would read as complete
                    // while configuration nested inside went unlooked-at. The
                    // budget used to announce itself when it ran out; this is
                    // that announcement, made where the decision is taken.
                    findings.push(finding(root, &path, GENERATED));
                } else {
                    pending.push(path);
                }
            }
        }
    }
    findings
}

/// What a run could execute from the project's agent configuration, as one
/// stable hash: every path the trust scan found, and the bytes of every file
/// under the ones that can run something (settings and hooks, MCP servers,
/// Codex config). Instruction files (`*.md`) are left out: they change with
/// ordinary work and run nothing. A different hash means the user trusted
/// something other than what is there now.
pub fn trust_fingerprint(root: &Path, findings: &[TrustFinding]) -> String {
    // FNV-1a: stable across builds, unlike the std hasher.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            hash = (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
    };
    // ponytail: first 2000 files under the config paths; a `.claude` bigger than that is not config.
    let mut files = 0;
    // A build folder a check just created is not new configuration: its top
    // level is still scanned, so a `.claude` put there is its own finding. It
    // once stopped a run between Verify and the Fix over a fresh `dist/`.
    for f in findings
        .iter()
        .filter(|f| !f.path.to_ascii_lowercase().ends_with(".md") && f.reason != GENERATED)
    {
        feed(f.path.as_bytes());
        feed(f.reason.as_bytes());
        // An ancestor's (absolute) path counts by name only: the home folder's
        // own `~/.claude` sits above most projects, is written by every CLI
        // session, and runs load no user settings anyway. Only config paths
        // are read: a skipped `target/` or `node_modules` is a finding too,
        // and walking one took seconds on every open.
        if Path::new(&f.path).is_absolute() || !TRUST_TARGETS.iter().any(|(_, r)| *r == f.reason) {
            continue;
        }
        let mut stack = vec![root.join(&f.path)];
        while let Some(path) = stack.pop() {
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            if meta.is_dir() {
                let mut children: Vec<PathBuf> = std::fs::read_dir(&path)
                    .map(|entries| entries.flatten().map(|e| e.path()).collect())
                    .unwrap_or_default();
                children.sort();
                stack.extend(children.into_iter().rev());
            } else if files < 2000 {
                files += 1;
                feed(path.strip_prefix(root).unwrap_or(&path).to_string_lossy().as_bytes());
                feed(&std::fs::read(&path).unwrap_or_default());
            }
        }
    }
    format!("{hash:016x}")
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

/// Only whether `dir` is in a repository, and its root. For a project nobody
/// has trusted yet: `git status` can run a clean filter the repository's own
/// config names, and `rev-parse` runs nothing.
pub fn git_root(dir: &Path) -> GitState {
    match git(dir, &["rev-parse", "--show-toplevel"]) {
        Some(root) => GitState { is_repo: true, root: Some(root), ..Default::default() },
        None => GitState::default(),
    }
}

pub fn git_state(dir: &Path) -> GitState {
    let Some(root) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return GitState::default();
    };
    // One status call answers branch, HEAD, upstream, ahead/behind and the
    // changes; asking each separately was five git starts on every preview.
    let status = git_output(dir, &["status", "--porcelain=v2", "--branch", "-z"])
        .map(|text| parse_status(&text))
        .unwrap_or_default();

    let branches = git(dir, &["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .unwrap_or_default()
        .lines()
        .filter(|b| Some(*b) != status.branch.as_deref())
        .map(String::from)
        .collect();

    let dirty_count = status.changes.len();
    GitState {
        branches,
        upstream: status.upstream,
        ahead: status.ahead,
        behind: status.behind,
        is_repo: true,
        root: Some(root),
        branch: status.branch,
        head: status.head,
        dirty: dirty_count > 0,
        dirty_count,
        changes: status.changes,
    }
}

fn git_changes(dir: &Path) -> Vec<GitChange> {
    git_output(dir, &["status", "--porcelain=v2", "-z"])
        .map(|text| parse_status(&text).changes)
        .unwrap_or_default()
}

/// What `git status --porcelain=v2 --branch -z` says.
#[derive(Debug, Default, PartialEq)]
struct Status {
    head: Option<String>,
    branch: Option<String>,
    /// Only when git could compare against it: a gone upstream is no upstream.
    upstream: Option<String>,
    ahead: Option<usize>,
    behind: Option<usize>,
    changes: Vec<GitChange>,
}

fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    let mut upstream = None;
    let mut records = text.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.into()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.into()),
                "branch.upstream" => upstream = Some(value.to_string()),
                "branch.ab" => {
                    let mut ab = value.split(' ').map(|n| n.trim_start_matches(['+', '-']).parse().ok());
                    status.ahead = ab.next().flatten();
                    status.behind = ab.next().flatten();
                }
                _ => {}
            }
            continue;
        }
        // Fields before the path: ordinary 8, rename/copy 9, unmerged 10.
        let (xy, path) = match record.as_bytes().first() {
            Some(b'1') => (record.get(2..4), record.splitn(9, ' ').nth(8)),
            Some(b'2') => (record.get(2..4), record.splitn(10, ' ').nth(9)),
            Some(b'u') => (record.get(2..4), record.splitn(11, ' ').nth(10)),
            Some(b'?') => (Some("??"), record.get(2..)),
            _ => continue,
        };
        // A rename's old path follows as its own record.
        if record.starts_with('2') {
            records.next();
        }
        if let (Some(xy), Some(path)) = (xy, path) {
            // v1's spelling, which the screen and `discard` read: `.` is a space.
            status.changes.push(GitChange { path: path.replace('\\', "/"), status: xy.replace('.', " ") });
        }
    }
    if status.ahead.is_some() {
        status.upstream = upstream;
    }
    status
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
    /// Already changed when the run started, and back to the commit after:
    /// the run undid the user's change. The diff alone would drop it silently.
    Reverted,
}

/// Every file that was already changed when a run started, with a fingerprint
/// of its contents. Taken before the first provider starts.
#[derive(Debug, Clone, Default, Hash, PartialEq)]
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
/// before it. A file that was dirty before and is no longer in the diff was
/// put back to the commit by the run, and is added as `Reverted`.
pub fn attribute(dir: &Path, diff: &mut Vec<FileStat>, before: Option<&Snapshot>) {
    let Some(before) = before else { return };
    for file in diff.iter_mut() {
        file.origin = Some(match before.0.iter().find(|(path, _)| *path == file.path) {
            None => Origin::Run,
            Some((_, print)) if *print == fingerprint(&dir.join(&file.path)) => Origin::BeforeRun,
            Some(_) => Origin::Both,
        });
    }
    let reverted: Vec<FileStat> = before
        .0
        .iter()
        .filter(|(path, _)| !diff.iter().any(|f| f.path == *path))
        .map(|(path, _)| FileStat { path: path.clone(), added: None, deleted: None, origin: Some(Origin::Reverted) })
        .collect();
    diff.extend(reverted);
}

/// Everything uncommitted, tracked and untracked, as a commit no branch holds,
/// kept by `refs/orteca/before/<task>/<ms>`. Built in a throwaway index, so the
/// working tree and the user's own index are not touched. What a run undoes
/// can be put back from it. `None` when git cannot make it.
pub fn save_state(dir: &Path, head: Option<&str>, task_id: i64) -> Option<String> {
    let index = std::env::temp_dir().join(format!("orteca-index-{task_id}-{}", std::process::id()));
    let run = |args: &[&str]| -> Option<String> {
        let out = Command::new("git")
            .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_TERMINAL_PROMPT", "0")
            // A machine with no git identity still gets its work saved.
            .env("GIT_AUTHOR_NAME", "Orteca")
            .env("GIT_AUTHOR_EMAIL", "orteca@localhost")
            .env("GIT_COMMITTER_NAME", "Orteca")
            .env("GIT_COMMITTER_EMAIL", "orteca@localhost")
            .creation_flags(CREATE_NO_WINDOW)
            .current_dir(dir)
            .output()
            .ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let saved = (|| {
        match head {
            Some(head) => run(&["read-tree", head])?,
            None => run(&["read-tree", "--empty"])?,
        };
        run(&["add", "-A"])?;
        let tree = run(&["write-tree"])?;
        let message = format!("Orteca: uncommitted work before task {task_id}");
        let mut args = vec!["commit-tree", "--no-gpg-sign", tree.as_str(), "-m", message.as_str()];
        if let Some(head) = head {
            args.extend(["-p", head]);
        }
        let commit = run(&args)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        run(&["update-ref", &format!("refs/orteca/before/{task_id}/{stamp}"), &commit])?;
        Some(commit)
    })();
    let _ = std::fs::remove_file(&index);
    saved
}

/// Put one file back as it was before a run, from the commit `save_state`
/// made. Only a file that is now exactly as committed, so nothing is lost.
pub fn restore_saved(dir: &Path, saved: &str, file: &str) -> Result<()> {
    if !(saved.len() >= 40 && saved.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(AppError::new(ErrorKind::Invalid, "That is not a saved state Orteca made."));
    }
    if file.starts_with('-') || git(dir, &["status", "--porcelain", "--", file]).is_some() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("{file} has changed since, so Orteca will not overwrite it."),
        ));
    }
    if git(dir, &["cat-file", "-t", &format!("{saved}:{file}")]).is_none() {
        // The user had deleted it and the run brought it back. It is tracked
        // and as committed, so removing it again loses nothing.
        if git(dir, &["ls-files", "--", file]).as_deref() != Some(file) {
            return Err(AppError::new(ErrorKind::NotFound, format!("{file} was not saved.")));
        }
        return std::fs::remove_file(dir.join(file)).map_err(Into::into);
    }
    git_run(dir, &["restore", &format!("--source={saved}"), "--worktree", "--", file], "Git could not restore the file")
        .map(drop)
}

/// Put files back as they were in `saved`, the state kept before one exchange
/// of a task: a file it holds is restored, one it does not is removed, since
/// it did not exist then. Whatever changed them since is lost, so the caller
/// has asked the user first.
pub fn rewind_files(dir: &Path, saved: &str, files: &[String]) -> Result<()> {
    if !(saved.len() >= 40 && saved.chars().all(|c| c.is_ascii_hexdigit())) {
        return Err(AppError::new(ErrorKind::Invalid, "That is not a saved state Orteca made."));
    }
    let outside = |f: &str| {
        f.starts_with('-') || Path::new(f).components().any(|c| !matches!(c, std::path::Component::Normal(_)))
    };
    if let Some(f) = files.iter().find(|f| outside(f)) {
        return Err(AppError::new(ErrorKind::Invalid, format!("{f} is not a file in this project.")));
    }
    let (kept, gone): (Vec<&String>, Vec<&String>) =
        files.iter().partition(|f| git(dir, &["cat-file", "-t", &format!("{saved}:{f}")]).is_some());
    if !kept.is_empty() {
        let source = format!("--source={saved}");
        let mut args = vec!["restore", source.as_str(), "--worktree", "--"];
        args.extend(kept.iter().map(|f| f.as_str()));
        git_run(dir, &args, "Git could not put the files back")?;
    }
    for f in gone {
        match std::fs::remove_file(dir.join(f)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
    }
    Ok(())
}

/// Forget the saved states of one task.
pub fn drop_saved(dir: &Path, task_id: i64) {
    let refs = git(dir, &["for-each-ref", "--format=%(refname)", &format!("refs/orteca/before/{task_id}/")]).unwrap_or_default();
    for name in refs.lines() {
        let _ = git_run(dir, &["update-ref", "-d", name], "");
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

/// One commit as the dock's history tab lists it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    /// Abbreviated, which is what a person reads and what `commit_patch` takes.
    pub hash: String,
    pub subject: String,
    pub author: String,
    /// Git's own wording - "3 hours ago". Not computed here, so it cannot drift.
    pub relative: String,
    /// ISO 8601, for the exact time behind the relative one.
    pub when: String,
}

/// `count` commits from `skip` back, newest first. A repository with no commits
/// yet has no HEAD to log, which is a normal state and reads as an empty list
/// rather than a failure.
pub fn git_log(dir: &Path, skip: u32, count: u32) -> Result<Vec<Commit>> {
    // Separators no commit message can contain, so a subject carrying a tab or
    // a newline still parses into the right field.
    let text = match git_output(
        dir,
        &[
            "log",
            "--no-show-signature",
            &format!("--skip={skip}"),
            &format!("--max-count={count}"),
            "--format=%h%x1f%an%x1f%ar%x1f%aI%x1f%s%x1e",
        ],
    ) {
        Ok(text) => text,
        Err(_) if git(dir, &["rev-parse", "--verify", "HEAD"]).is_none() => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(parse_log(&text))
}

fn parse_log(text: &str) -> Vec<Commit> {
    text.split('\u{1e}')
        .map(str::trim_start)
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let mut fields = record.split('\u{1f}');
            Some(Commit {
                hash: fields.next()?.to_string(),
                author: fields.next()?.to_string(),
                relative: fields.next()?.to_string(),
                when: fields.next()?.to_string(),
                subject: fields.next()?.to_string(),
            })
        })
        .collect()
}

/// What one commit changed. `hash` is checked to be a hexadecimal name before
/// it reaches git, so nothing the frontend sends can arrive as an option.
pub fn commit_patch(dir: &Path, hash: &str) -> Result<String> {
    const MAX_PATCH_BYTES: usize = 512 * 1024;
    if hash.len() < 4 || hash.len() > 40 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::new(ErrorKind::Invalid, "That is not a commit name."));
    }
    // `show`, not `diff`: the first commit in a repository has no parent.
    let mut patch = git_output(
        dir,
        &["show", "--no-ext-diff", "--no-textconv", "--format=", hash],
    )?;
    if patch.len() > MAX_PATCH_BYTES {
        truncate_utf8(&mut patch, MAX_PATCH_BYTES);
        patch.push_str("\n… patch truncated\n");
    }
    Ok(patch)
}

/// What the working tree holds that HEAD does not - the changes no commit owns
/// yet. Before the first commit there is no HEAD, and everything in the tree is
/// the uncommitted change.
pub fn working_patch(dir: &Path) -> Result<String> {
    let head = git(dir, &["rev-parse", "--verify", "HEAD"]).is_some();
    patch_since(dir, head.then_some("HEAD"))
}

/// The reviewable patch for the same comparison as `diff_since`.
///
/// Git does not include untracked files in `git diff`, so small text files are
/// rendered as new-file hunks here. Large and binary untracked files remain in
/// the file-stat list but are called out instead of loading unbounded data.
pub fn patch_since(dir: &Path, base: Option<&str>) -> Result<String> {
    patch_of(dir, base, None)
}

/// The same patch narrowed to `only`, for a caller that knows which paths it is
/// asking about. `None` is every path; an empty slice is no path and yields an
/// empty patch, which is the honest answer to "show me nothing".
pub fn patch_of(dir: &Path, base: Option<&str>, only: Option<&[String]>) -> Result<String> {
    const MAX_PATCH_BYTES: usize = 512 * 1024;
    const MAX_UNTRACKED_BYTES: usize = 64 * 1024;
    if only.is_some_and(<[String]>::is_empty) {
        return Ok(String::new());
    }
    let empty_tree;
    let base = match base {
        Some(base) => base,
        None => {
            empty_tree = git_output(dir, &["hash-object", "-t", "tree", "--stdin"])?;
            empty_tree.trim()
        }
    };
    let mut argv = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--binary",
        "--full-index",
        base,
        // Everything after this is a pathspec, so a path that starts with a
        // dash cannot be read as a flag.
        "--",
    ];
    argv.extend(only.unwrap_or_default().iter().map(String::as_str));
    let mut patch = git_output(dir, &argv)?;
    let untracked = git_output(dir, &["ls-files", "-z", "--others", "--exclude-standard"])?;
    for path in untracked
        .split('\0')
        .filter(|path| !path.is_empty())
        .filter(|path| only.map_or(true, |only| only.iter().any(|kept| kept == path)))
    {
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
        "\ndiff --git a/{path} b/{path}\nnew file mode 100644\n"
    );
    if count == 0 {
        return patch;
    }
    patch.push_str(&format!(
        "--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{count} @@\n"
    ));
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    if !text.ends_with('\n') {
        patch.push_str("\\ No newline at end of file\n");
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
                && !GENERATED_DIRS.contains(&name.as_str())
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
    let kinds = changed_kinds(changed_paths);
    if kinds.is_empty() {
        return checks;
    }
    checks
        .into_iter()
        .filter(|check| kinds.contains(check.kind))
        .collect()
}

/// The ecosystems these changed paths belong to: `php`, `js`, `rust`, `go`, `python`.
fn changed_kinds(changed_paths: &[String]) -> std::collections::HashSet<&'static str> {
    changed_paths
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
        .collect()
}

/// Build and lint commands the user explicitly requested, limited to scripts
/// the repository itself declares. They run as Orteca in a trusted project,
/// never as an arbitrary command invented by a model.
pub fn requested_check_commands(root: &Path, build: bool, lint: bool) -> Vec<Check> {
    let names: Vec<&'static str> = [(build, "build"), (lint, "lint")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect();
    script_checks(root, &names)
}

/// What checks a code change or fix after its tests: the repository's own
/// lint, type-check and build scripts, where it declares them.
// ponytail: package.json scripts only; add phpstan or clippy when a project wants them.
/// Only for a change that touches JS: `npm run build` on a PHP-only change
/// cost 26-75 s a Verify on the RigInspect benchmark and could fail nothing.
pub fn gate_commands(root: &Path, changed_paths: &[String]) -> Vec<Check> {
    if !changed_kinds(changed_paths).contains("js") {
        return Vec::new();
    }
    script_checks(root, &["lint", "typecheck", "type-check", "check-types", "build"])
}

/// A check per declared script in `names`, run the lockfile's way.
fn script_checks(root: &Path, names: &[&'static str]) -> Vec<Check> {
    if names.is_empty() {
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
    for script in names.iter().copied() {
        if scripts.get(script).and_then(|value| value.as_str()).is_some() {
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

/// Folders and files an agent has no reason to read: dependencies, build
/// output, caches, lockfiles and minified bundles. `storage/framework/` and
/// not `storage/`, because a debug task reads `storage/logs/`.
pub const JUNK_DIRS: &[&str] = &[
    "node_modules", "vendor", "storage/framework", "bootstrap/cache", "dist", "build", "target",
    "public/build", ".next", "coverage",
];
pub const JUNK_FILES: &[&str] = &["*.lock", "package-lock.json", "*.min.js", "*.min.css", "*.map"];

/// The junk folders this repository has, and the junk file patterns.
pub fn junk(root: &Path) -> (Vec<&'static str>, &'static [&'static str]) {
    (JUNK_DIRS.iter().copied().filter(|d| root.join(d).is_dir()).collect(), JUNK_FILES)
}

/// A few static lines on how this repository is built, tested and laid out,
/// and what not to read. Sent with every run's rules unless the user wrote
/// their own. Read from manifests only; nothing is run.
pub fn profile(root: &Path) -> String {
    let mut lines = Vec::new();
    let has = |file: &str| root.join(file).is_file();
    if has("artisan") {
        lines.push("Laravel app: run it with `php artisan serve`.".to_string());
    }
    for check in check_commands(root) {
        let at = check.dir.strip_prefix(root).ok().filter(|p| !p.as_os_str().is_empty());
        let at = at.map_or(String::new(), |p| format!(" (in {}/)", p.to_string_lossy().replace('\\', "/")));
        lines.push(format!("Test: `{}`{at}", check.test.join(" ")));
    }
    let scripts = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|m| m["scripts"].as_object().cloned())
        .unwrap_or_default();
    let run = js_runner(root);
    for (label, names) in [
        ("Build", &["build"][..]),
        ("Lint", &["lint"]),
        ("Type check", &["typecheck", "type-check", "check-types"]),
        ("Dev server", &["dev", "start"]),
    ] {
        if let Some(name) = names.iter().find(|n| scripts.contains_key(**n)) {
            lines.push(format!("{label}: `{run} run {name}`"));
        }
    }
    if has("Cargo.toml") {
        lines.push("Build: `cargo build`".into());
    }
    let mut folders: Vec<String> = tracked_paths(root)
        .iter()
        .filter_map(|p| p.split_once('/').map(|(dir, _)| dir.to_string()))
        .filter(|d| !d.starts_with('.'))
        .collect();
    folders.sort();
    folders.dedup();
    if !folders.is_empty() {
        let shown: Vec<String> = folders.iter().take(12).map(|d| format!("{d}/")).collect();
        lines.push(format!("Top folders: {}", shown.join(", ")));
    }
    let (dirs, files) = junk(root);
    let skip: Vec<String> = dirs.iter().map(|d| format!("{d}/")).chain(files.iter().map(|f| f.to_string())).collect();
    lines.push(format!("Do not read: {}", skip.join(", ")));
    lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")
}

/// The package manager a JS repository's lockfile names.
fn js_runner(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lock").is_file() || root.join("bun.lockb").is_file() {
        "bun"
    } else {
        "npm"
    }
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

/// Files changed together in the last 300 commits: for each file, the ones
/// that shared at least two commits with it, most often first, and how many
/// commits were read. A commit of more than 20 files says nothing about pairs.
pub fn co_changes(dir: &Path) -> (usize, std::collections::HashMap<String, Vec<(String, u32)>>) {
    use std::collections::HashMap;
    let log = git_output(dir, &["-c", "core.quotepath=false", "log", "-n", "300", "--name-only", "--format=%x00"])
        .unwrap_or_default();
    let mut pairs: HashMap<(&str, &str), u32> = HashMap::new();
    let mut commits = 0;
    for commit in log.split('\0') {
        let files: Vec<&str> = commit.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        if files.is_empty() {
            continue;
        }
        commits += 1;
        if files.len() > 20 {
            continue;
        }
        for (i, a) in files.iter().enumerate() {
            for b in &files[i + 1..] {
                *pairs.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut map: HashMap<String, Vec<(String, u32)>> = HashMap::new();
    for ((a, b), n) in pairs.into_iter().filter(|(_, n)| *n >= 2) {
        map.entry(a.to_string()).or_default().push((b.to_string(), n));
        map.entry(b.to_string()).or_default().push((a.to_string(), n));
    }
    for list in map.values_mut() {
        list.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
        list.truncate(5);
    }
    (commits, map)
}

/// How far co-changes are trusted, from the commits `co_changes` read (at
/// most 300): not at all under 150, fully at 300. Measured with `navstats
/// --recall`: on 119 commits (LiftMe) they cost 73% -> 68%, on 300 of 695
/// (RigInspect) they gave 61% -> 68%.
// ponytail: two repositories set these bounds; move them when more runs say so.
pub fn co_change_weight(commits: usize) -> f64 {
    if commits < 150 { 0.0 } else { (commits as f64 / 300.0).min(1.0) }
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

/// A file's committed text at `commit`, `None` when it was not there. `path`
/// is relative to the repository root.
pub fn file_at(repo: &Path, commit: &str, path: &Path) -> Option<String> {
    let spec = format!("{commit}:{}", path.to_string_lossy().replace('\\', "/"));
    git_output(repo, &["cat-file", "blob", &spec]).ok()
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

/// What a run leaves in the temp folder when Orteca dies mid-way: a base copy
/// (with the project's `.env` in it) and `save_state`'s throwaway index.
/// Swept at start-up, when no run can still be using them. The repository's
/// note of the copy goes at its next `worktree prune`.
pub fn sweep_temp() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("orteca-base-") {
            let _ = std::fs::remove_dir_all(entry.path());
        } else if name.starts_with("orteca-index-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Runs as the user, with their identity and their hooks: this is their
/// commit, not a run's. `input` is the commit message, the branch to merge or
/// switch to, or the name of a new branch.
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
        GitAction::Discard => discard(dir, input),
        // `switch` carries uncommitted work along and refuses when it would
        // overwrite any, so nothing of the user's is lost either way.
        GitAction::Switch => {
            if !git_state(dir).branches.iter().any(|b| b == input) {
                return Err(AppError::new(ErrorKind::Invalid, "That branch does not exist here."));
            }
            git_run(dir, &["switch", input], "Git could not switch branch")
        }
        GitAction::Branch => {
            let name = input.trim();
            if name.starts_with('-') || git(dir, &["check-ref-format", "--branch", name]).is_none() {
                return Err(AppError::new(ErrorKind::Invalid, "That is not a valid branch name."));
            }
            git_run(dir, &["switch", "-c", name], "Git could not create the branch")
        }
    }
    .map(drop)
}

/// Revert one selected path, or every Git-visible path when `path` is empty.
/// The UI confirms this destructive action and the backend re-checks that the
/// requested path is currently changed, so arbitrary paths never reach Git.
fn discard(dir: &Path, path: &str) -> Result<String> {
    let changes = git_changes(dir);
    let selected: Vec<&GitChange> = if path.is_empty() {
        changes.iter().collect()
    } else {
        changes.iter().filter(|change| change.path == path).collect()
    };
    if selected.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "That changed file is no longer available to revert."));
    }
    for change in selected {
        if change.status == "??" {
            git_run(dir, &["clean", "-f", "--", &change.path], "Git could not remove the untracked file")?;
        } else if git(dir, &["rev-parse", "HEAD"]).is_none() {
            // An unborn branch has no HEAD to restore from. Unstage first so
            // the file becomes untracked, then remove it with Git as usual.
            git_run(dir, &["rm", "--cached", "--", &change.path], "Git could not unstage the file")?;
            git_run(dir, &["clean", "-f", "--", &change.path], "Git could not remove the untracked file")?;
        } else {
            git_run(dir, &["restore", "--source=HEAD", "--staged", "--worktree", "--", &change.path], "Git could not revert the file")?;
        }
    }
    Ok(String::new())
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

/// The lines of the repo that mention what a picked element shows: its text,
/// its class list, its id. Best match first, at most five, as `file:line: text`.
pub fn find_lines(dir: &Path, needles: &[String]) -> Vec<String> {
    let needles: Vec<&str> = needles.iter().map(|n| n.trim()).filter(|n| n.len() >= 3).take(4).collect();
    if needles.is_empty() {
        return Vec::new();
    }
    let mut args = vec!["grep", "-n", "-I", "-F", "--untracked", "--exclude-standard"];
    for n in &needles {
        args.extend(["-e", n]);
    }
    rank_hits(&git(dir, &args).unwrap_or_default(), &needles)
}

/// `git grep -n` output ranked by how many distinct needles a line holds.
/// A minified line is skipped: it matches everything and helps nobody.
fn rank_hits(output: &str, needles: &[&str]) -> Vec<String> {
    let mut hits: Vec<(usize, &str)> = output
        .lines()
        .filter(|l| l.len() < 400)
        .map(|l| (needles.iter().filter(|n| l.contains(**n)).count(), l))
        .collect();
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.0)); // stable: ties keep git's file order
    hits.into_iter()
        .take(5)
        .map(|(_, l)| clip(l))
        .collect()
}

/// A long line keeps both ends: a tag opens with its classes, but the handler
/// and the text that tell one twin from another come last.
fn clip(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= 170 {
        return line.to_string();
    }
    format!("{} … {}", chars[..70].iter().collect::<String>(), chars[chars.len() - 90..].iter().collect::<String>())
}

/// Where a run keeps its own files: inside the git directory, which git never
/// lists as a change. A copy's git directory is its own, beside the main one.
pub fn orteca_dir(dir: &Path) -> Option<PathBuf> {
    Some(PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"])?).join("orteca"))
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

    /// Bench helper (scripts/bench/pick.mjs): the real search, on a bench worktree.
    #[test]
    #[ignore = "BENCH_DIR + BENCH_NEEDLES (JSON array); prints FOUND <json>"]
    fn bench_find() {
        let dir = std::env::var("BENCH_DIR").unwrap();
        let needles: Vec<String> = serde_json::from_str(&std::env::var("BENCH_NEEDLES").unwrap()).unwrap();
        println!("FOUND {}", serde_json::to_string(&find_lines(Path::new(&dir), &needles)).unwrap());
    }

    #[test]
    fn one_status_call_reads_branch_upstream_and_every_kind_of_change() {
        let text = [
            "# branch.oid 1234abcd",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
            "1 .M N... 100644 100644 100644 aaa bbb src/a file.rs",
            "2 R. N... 100644 100644 100644 aaa bbb R100 new.rs",
            "old.rs",
            "u UU N... 100644 100644 100644 100644 aaa bbb ccc both.rs",
            "? notes.txt",
            "",
        ]
        .join("\0");
        let status = parse_status(&text);
        assert_eq!(status.head.as_deref(), Some("1234abcd"));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (Some(2), Some(3)));
        let changes: Vec<(&str, &str)> =
            status.changes.iter().map(|c| (c.path.as_str(), c.status.as_str())).collect();
        assert_eq!(
            changes,
            [("src/a file.rs", " M"), ("new.rs", "R "), ("both.rs", "UU"), ("notes.txt", "??")]
        );

        // No commits, detached, or an upstream git cannot compare against.
        let bare = parse_status("# branch.oid (initial)\0# branch.head (detached)\0# branch.upstream origin/gone\0");
        assert_eq!(bare, Status::default());
    }

    #[test]
    fn hits_rank_by_how_many_needles_a_line_holds() {
        let out = "a.vue:3:<p>Buy now</p>\nb.vue:9:<button class=\"btn\">Buy now</button>";
        let ranked = rank_hits(out, &["Buy now", "class=\"btn\""]);
        assert!(ranked[0].starts_with("b.vue:9:"));
        assert_eq!(ranked.len(), 2);
    }

    #[test]
    fn a_commit_subject_carrying_a_tab_or_a_newline_still_parses() {
        // Exactly what the `--format` in `git_log` writes, for two commits.
        let text = "4dbd4fc\u{1f}Aleksis\u{1f}2 hours ago\u{1f}2026-09-19T21:04:00+03:00\u{1f}Name the\tstub\u{1e}\nbaaa5ad\u{1f}Aleksis\u{1f}5 hours ago\u{1f}2026-09-19T18:00:00+03:00\u{1f}Bound what\nthe classifier reasons\u{1e}\n";
        let log = parse_log(text);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].hash, "4dbd4fc");
        assert_eq!(log[0].author, "Aleksis");
        assert_eq!(log[0].relative, "2 hours ago");
        assert_eq!(log[0].subject, "Name the\tstub");
        assert_eq!(log[1].subject, "Bound what\nthe classifier reasons");
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn a_commit_name_that_is_not_hexadecimal_never_reaches_git() {
        let dir = std::env::current_dir().unwrap();
        for bad in ["--upload-pack=calc", "HEAD", "abc", "../etc", ""] {
            assert!(commit_patch(&dir, bad).is_err(), "{bad}");
        }
    }

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
    fn branches_are_created_and_switched_without_losing_work() {
        let dir = temp_dir("git-switch");
        let run = |args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(&dir).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "test@example.com"]);
        std::fs::write(dir.join("a.txt"), "base\n").unwrap();
        git_action(&dir, GitAction::Commit, "base").unwrap();

        for bad in ["", "-f", "two words", "a..b"] {
            assert!(git_action(&dir, GitAction::Branch, bad).is_err(), "created {bad:?}");
        }
        git_action(&dir, GitAction::Branch, " feature ").unwrap();
        assert_eq!(git_state(&dir).branch.as_deref(), Some("feature"));
        assert!(git_action(&dir, GitAction::Branch, "main").is_err(), "an existing branch was recreated");

        assert!(git_action(&dir, GitAction::Switch, "--help").is_err());
        std::fs::write(dir.join("a.txt"), "unsaved\n").unwrap();
        git_action(&dir, GitAction::Switch, "main").unwrap();
        let state = git_state(&dir);
        assert_eq!((state.branch.as_deref(), state.dirty), (Some("main"), true), "uncommitted work was not carried");
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "unsaved\n");
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
    fn new_file_patch_uses_real_line_ranges_and_preserves_the_missing_newline() {
        let patch = new_file_patch("new.txt", "first\nsecond");
        assert!(patch.contains("@@ -0,0 +1,2 @@\n+first\n+second\n"));
        assert!(patch.ends_with("\\ No newline at end of file\n"));
        assert!(!new_file_patch("new.txt", "first\n").contains("No newline"));
        let empty = new_file_patch("empty.txt", "");
        assert!(empty.contains("new file mode 100644"));
        assert!(!empty.contains("@@"));
    }

    #[test]
    fn the_working_patch_holds_what_no_commit_does_yet() {
        let dir = temp_dir("working-patch");
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
        std::fs::write(dir.join("tracked.txt"), "before
").unwrap();
        // Before the first commit there is no HEAD, and the whole tree is new.
        assert!(working_patch(&dir).unwrap().contains("+before"));
        command(&["add", "."]);
        command(&["commit", "-qm", "initial"]);
        assert_eq!(working_patch(&dir).unwrap().trim(), "");
        std::fs::write(dir.join("tracked.txt"), "after
").unwrap();
        std::fs::write(dir.join("untracked.txt"), "new
").unwrap();
        let patch = working_patch(&dir).unwrap();
        assert!(patch.contains("-before") && patch.contains("+after"), "{patch}");
        assert!(patch.contains("b/untracked.txt"), "{patch}");
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
    /// Rewinding puts the named files back as the saved state had them: the
    /// user's uncommitted edit comes back, a file made since goes, and a
    /// file not named is left alone.
    #[test]
    fn rewinding_puts_named_files_back_as_saved() {
        let dir = temp_dir("rewind");
        let command = |args: &[&str]| {
            let out = Command::new("git").args(args).current_dir(&dir).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        };
        command(&["init", "-q"]);
        command(&["config", "user.name", "test"]);
        command(&["config", "user.email", "test@example.com"]);
        std::fs::write(dir.join("a.txt"), "old\n").unwrap();
        std::fs::write(dir.join("b.txt"), "old\n").unwrap();
        command(&["add", "."]);
        command(&["commit", "-qm", "initial"]);
        let base = git_state(&dir).head.unwrap();
        std::fs::write(dir.join("a.txt"), "old\nmine\n").unwrap();
        let saved = save_state(&dir, Some(&base), 9).expect("saved state");

        // Two answers later.
        std::fs::write(dir.join("a.txt"), "agent\n").unwrap();
        std::fs::write(dir.join("new.txt"), "agent\n").unwrap();
        std::fs::write(dir.join("b.txt"), "user, later\n").unwrap();

        let files = ["a.txt".to_string(), "new.txt".to_string()];
        rewind_files(&dir, &saved, &files).unwrap();
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap().replace('\r', "");
        assert_eq!(read("a.txt"), "old\nmine\n");
        assert!(!dir.join("new.txt").exists(), "a file made after was kept");
        assert_eq!(read("b.txt"), "user, later\n", "a file not named was touched");
        // A clean tree's saved state is its commit.
        rewind_files(&dir, &base, &files).unwrap();
        assert_eq!(read("a.txt"), "old\n");
        assert!(rewind_files(&dir, &saved, &["../x".to_string()]).is_err());
        assert!(rewind_files(&dir, "HEAD", &files).is_err());
        drop_saved(&dir, 9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Fix is judged by the snapshot, which has no size cap: behind 600 KB
    /// of rebuilt assets the patch text stops before the file the Fix edited.
    #[test]
    fn a_snapshot_sees_an_edit_the_capped_patch_hides() {
        let dir = temp_dir("snapshot-cap");
        let git = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&dir).status().unwrap().success());
        git(&["init", "-q"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::create_dir_all(dir.join("public")).unwrap();
        std::fs::write(dir.join("public/app.js"), "old\n").unwrap();
        std::fs::write(dir.join("src.txt"), "old\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "initial"]);
        let base = git_state(&dir).head.unwrap();
        std::fs::write(dir.join("public/app.js"), "x\n".repeat(300_000)).unwrap();
        std::fs::write(dir.join("src.txt"), "first fix\n").unwrap();
        let (patch, before) = (patch_since(&dir, Some(&base)).unwrap(), snapshot(&dir, Some(&base)));
        std::fs::write(dir.join("src.txt"), "second fix\n").unwrap();
        assert_eq!(patch_since(&dir, Some(&base)).unwrap(), patch, "the capped patch hides the edit");
        assert_ne!(snapshot(&dir, Some(&base)), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        for name in ["clean.txt", "edited.txt", "gone.txt", "undone.txt"] {
            std::fs::write(dir.join(name), "old\n").unwrap();
        }
        command(&["add", "."]);
        command(&["commit", "-qm", "initial"]);
        let base = git_state(&dir).head.unwrap();

        // The user's own changes, before any run.
        std::fs::write(dir.join("stray.txt"), "untracked\n").unwrap();
        std::fs::write(dir.join("edited.txt"), "old\nmine\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("undone.txt"), "old\nmine too\n").unwrap();
        let before = snapshot(&dir, Some(&base)).expect("snapshot");
        let saved = save_state(&dir, Some(&base), 7).expect("saved state");
        assert!(git_state(&dir).dirty, "saving touched the working tree");

        // The run: touches a clean file and one the user had already edited.
        std::fs::write(dir.join("clean.txt"), "old\nagent\n").unwrap();
        std::fs::write(dir.join("edited.txt"), "old\nmine\nagent\n").unwrap();
        std::fs::write(dir.join("created.txt"), "agent\n").unwrap();
        // ...and undoes one of the user's.
        std::fs::write(dir.join("undone.txt"), "old\n").unwrap();

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
        assert_eq!(origin("undone.txt"), Some(Origin::Reverted), "an undone change went unreported");
        // What the run undid comes back from the saved state; nothing else moves.
        restore_saved(&dir, &saved, "undone.txt").unwrap();
        // Line endings follow the repo's checkout settings, as `git stash` would.
        assert_eq!(std::fs::read_to_string(dir.join("undone.txt")).unwrap().replace('\r', ""), "old\nmine too\n");
        assert!(restore_saved(&dir, &saved, "clean.txt").is_err(), "a changed file was overwritten");
        drop_saved(&dir, 7);
        assert!(git(&dir, &["for-each-ref", "refs/orteca/"]).is_none());
        std::fs::write(dir.join("undone.txt"), "old\n").unwrap();
        diff.retain(|f| f.path != "undone.txt");

        // No snapshot means no claim either way.
        let mut unknown = diff_since(&dir, Some(&base)).unwrap();
        attribute(&dir, &mut unknown, None);
        assert!(unknown.iter().all(|f| f.origin.is_none()));

        // What a Review is shown: the run's work, and not the user's own edits
        // from before it. A reviewer handed both reports the user's as an
        // unrequested change and holds up work that never touched it.
        let ours: Vec<String> = diff
            .iter()
            .filter(|f| f.origin != Some(Origin::BeforeRun))
            .map(|f| f.path.clone())
            .collect();
        let patch = patch_of(&dir, Some(&base), Some(&ours)).unwrap();
        for ran in ["clean.txt", "edited.txt", "created.txt"] {
            assert!(patch.contains(ran), "{ran} missing from\n{patch}");
        }
        for theirs in ["stray.txt", "gone.txt"] {
            assert!(!patch.contains(theirs), "{theirs} shown in\n{patch}");
        }
        // Untracked included: a new file the run wrote is the run's work.
        assert!(patch.contains("agent\n"), "{patch}");
        // No path is no patch, and no filter is every path.
        assert_eq!(patch_of(&dir, Some(&base), Some(&[])).unwrap(), "");
        assert!(patch_since(&dir, Some(&base)).unwrap().contains("stray.txt"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Trust follows what can run: a hook edit asks again, an instructions edit does not.
    #[test]
    fn the_trust_fingerprint_moves_with_runnable_config_only() {
        let dir = temp_dir("trust-print");
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(".claude/settings.json"), "{}").unwrap();
        std::fs::write(dir.join("AGENTS.md"), "be brief").unwrap();
        let print = || trust_fingerprint(&dir, &trust_scan(&dir));
        let first = print();
        assert_eq!(print(), first, "the same folder hashed differently");
        std::fs::write(dir.join("AGENTS.md"), "be very brief").unwrap();
        assert_eq!(print(), first, "an instructions edit asked for trust again");
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        assert_eq!(print(), first, "a build folder a check created asked for trust again");
        std::fs::create_dir_all(dir.join("dist/.claude")).unwrap();
        assert_ne!(print(), first, "config inside a build folder went unnoticed");
        std::fs::remove_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join(".claude/settings.json"), r#"{"hooks":{}}"#).unwrap();
        assert_ne!(print(), first, "a hook edit went unnoticed");
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

    /// A build folder must not cost the scan the rest of the repository: its
    /// own top level is still read, but nothing under it is walked, so the
    /// source beside it is always reached.
    #[test]
    fn the_trust_scan_does_not_walk_generated_folders() {
        let root = temp_dir("trust-generated");
        for file in [
            root.join("node_modules/CLAUDE.md"),
            root.join("target/deep/CLAUDE.md"),
            root.join("src/.claude/settings.json"),
        ] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "instructions").unwrap();
        }
        let found = |rel: &str| {
            trust_scan(&root)
                .iter()
                .any(|finding| finding.path.replace('\\', "/").starts_with(rel))
        };
        assert!(found("node_modules/CLAUDE.md"), "the folder's own top level is scanned");
        assert!(found("src/.claude"), "source beside a build folder is scanned");
        assert!(!found("target/deep"), "a build folder was walked");

        // Not walking it is a saving; not saying so would be a silent gap, and
        // consent given against a list that looks complete is not informed.
        let skipped: Vec<String> = trust_scan(&root)
            .iter()
            .filter(|f| f.reason.starts_with("generated directory"))
            .map(|f| f.path.replace('\\', "/"))
            .collect();
        for dir in ["node_modules", "target"] {
            assert!(
                skipped.iter().any(|s| s == dir),
                "{dir} was skipped without saying so: {skipped:?}"
            );
        }
        assert!(
            !skipped.iter().any(|s| s == "src"),
            "a walked folder claimed to be skipped"
        );
        std::fs::remove_dir_all(root).unwrap();
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
        assert_eq!(dirty.changes, [GitChange { path: "a.txt".into(), status: " M".into() }]);

        run(&["checkout", "--detach", "-q"]);
        assert!(git_state(&dir).branch.is_none());
        assert!(git_state(&dir).head.is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn discard_reverts_one_file_or_every_changed_file() {
        let dir = temp_dir("git-discard");
        let run = |args: &[&str]| {
            assert!(Command::new("git").args(args).current_dir(&dir).status().unwrap().success());
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("tracked.txt"), "base").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "base"]);
        std::fs::write(dir.join("tracked.txt"), "changed").unwrap();
        std::fs::write(dir.join("new.txt"), "new").unwrap();

        git_action(&dir, GitAction::Discard, "tracked.txt").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("tracked.txt")).unwrap(), "base");
        assert!(dir.join("new.txt").exists());

        git_action(&dir, GitAction::Discard, "").unwrap();
        assert!(!dir.join("new.txt").exists());
        assert!(!git_state(&dir).dirty);
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

