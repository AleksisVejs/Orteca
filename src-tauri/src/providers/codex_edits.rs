//! Per-edit patches for `codex exec`, whose file-change events omit the diff.
//! Checkpoints contain the working files, including the user's uncommitted edits.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

use super::ProviderEvent;

const MAX_FILE: usize = 2 * 1024 * 1024;
const MAX_SNAPSHOT: usize = 64 * 1024 * 1024;

pub struct EditSnapshots {
    root: PathBuf,
    files: HashMap<String, Option<String>>,
    bytes: usize,
    valid: bool,
}

impl EditSnapshots {
    pub fn capture(dir: &Path) -> Option<Self> {
        let root = dir.canonicalize().ok()?;
        let listed = git(
            &root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
        )?;
        if !listed.status.success() {
            return None;
        }
        let mut snapshot = Self {
            root,
            files: HashMap::new(),
            bytes: 0,
            valid: true,
        };
        let mut remaining = MAX_SNAPSHOT;
        for relative in std::str::from_utf8(&listed.stdout)
            .ok()?
            .split('\0')
            .filter(|s| !s.is_empty())
        {
            let Some(path) = snapshot.inside(relative) else {
                continue;
            };
            let key = key(&path);
            if snapshot.files.contains_key(&key) {
                continue;
            }
            let text = read_text(&path).filter(|text| text.len() <= remaining);
            remaining -= text.as_ref().map_or(0, String::len);
            snapshot.files.insert(key, text);
        }
        snapshot.bytes = MAX_SNAPSHOT - remaining;
        Some(snapshot)
    }

    /// Called before the completed item is recorded or emitted to the UI.
    pub fn complete(&mut self, raw: &Value, events: &mut [ProviderEvent]) {
        if raw["type"] != "item.completed" {
            return;
        }
        let item = &raw["item"];
        if item["type"] == "command_execution" {
            // A shell may write too. Its changes must not be charged to a later Edit.
            if let Some(snapshot) = Self::capture(&self.root) {
                *self = snapshot;
            } else {
                self.valid = false;
            }
            return;
        }
        if !self.valid || item["type"] != "file_change" {
            return;
        }
        let Some(changes) = item["changes"].as_array() else {
            return;
        };
        for change in changes {
            let Some(name) = change["path"].as_str() else {
                continue;
            };
            let Some(path) = self.inside(name) else {
                continue;
            };
            let key = key(&path);
            self.bytes -= self
                .files
                .get(&key)
                .and_then(Option::as_ref)
                .map_or(0, String::len);
            let after = read_text(&path).filter(|text| text.len() <= MAX_SNAPSHOT - self.bytes);
            self.bytes += after.as_ref().map_or(0, String::len);
            let before = self.files.get(&key).cloned().unwrap_or_else(|| {
                // Only an added, non-ignored file can be known absent from the
                // initial inventory. Ignored files and nested repos stay unknown.
                if change["kind"] != "add" {
                    return None;
                }
                if path
                    .ancestors()
                    .skip(1)
                    .any(|parent| self.files.contains_key(&self::key(parent)))
                {
                    return None;
                }
                let relative = path.strip_prefix(&self.root).ok()?.to_str()?;
                let ignored = git(&self.root, &["check-ignore", "--", relative])?;
                (ignored.status.code() == Some(1)).then(String::new)
            });
            let patch = before
                .as_deref()
                .zip(after.as_deref())
                .and_then(|(before, after)| diff(before, after));
            for event in events.iter_mut() {
                if let ProviderEvent::ToolUse { changes, .. } = event {
                    if let Some(edit) = changes
                        .iter_mut()
                        .find(|edit| edit.path == name && edit.patch.is_none())
                    {
                        edit.patch = patch.clone();
                    }
                }
            }
            self.files.insert(key, after);
        }
    }

    fn inside(&self, name: &str) -> Option<PathBuf> {
        let path = self.root.join(name);
        if fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return None;
        }
        let resolved = path
            .canonicalize()
            .ok()
            .or_else(|| Some(path.parent()?.canonicalize().ok()?.join(path.file_name()?)))?;
        resolved.starts_with(&self.root).then_some(resolved)
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

fn read_text(path: &Path) -> Option<String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(String::new()),
        Err(_) => return None,
    };
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_FILE as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take((MAX_FILE + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_FILE || bytes.contains(&0) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn git(dir: &Path, args: &[&str]) -> Option<Output> {
    Command::new("git")
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .creation_flags(0x0800_0000)
        .output()
        .ok()
}

fn diff(before: &str, after: &str) -> Option<String> {
    if before == after {
        return Some(String::new());
    }
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let scratch = std::env::temp_dir().join(format!(
        "orteca-edit-{}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&scratch).ok()?;
    let old = scratch.join("before");
    let new = scratch.join("after");
    let patch = (|| {
        fs::write(&old, before).ok()?;
        fs::write(&new, after).ok()?;
        let output = git(
            &scratch,
            &[
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--text",
                "--no-color",
                "--unified=3",
                "--",
                "before",
                "after",
            ],
        )?;
        if output.status.code() != Some(1) {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let start = text.find("\n@@ ")? + 1;
        Some(text[start..].to_string())
    })();
    let _ = fs::remove_file(old);
    let _ = fs::remove_file(new);
    let _ = fs::remove_dir(scratch);
    patch
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn repo(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("orteca-edit-test-{label}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert!(git(&dir, &["init", "-q"]).unwrap().status.success());
        dir
    }

    fn completed(
        snapshot: &mut EditSnapshots,
        path: &str,
        kind: &str,
        status: &str,
    ) -> Vec<ProviderEvent> {
        let raw = json!({"type":"item.completed", "item":{
            "id":"item_1", "type":"file_change", "status":status,
            "changes":[{"path":path, "kind":kind}]
        }});
        let mut events = super::super::codex::parse_line(&raw);
        snapshot.complete(&raw, &mut events);
        events
    }

    fn patch(events: &[ProviderEvent]) -> Option<&str> {
        let ProviderEvent::ToolUse { changes, .. } = &events[0] else {
            panic!("expected edit")
        };
        changes.first().and_then(|change| change.patch.as_deref())
    }

    #[test]
    fn each_edit_compares_with_the_previous_contents_not_head() {
        let dir = repo("repeated");
        fs::write(dir.join("state.ts"), "const value = 'already dirty';\n").unwrap();
        let mut snapshots = EditSnapshots::capture(&dir).unwrap();
        fs::write(dir.join("state.ts"), "const value = 'first edit';\n").unwrap();
        let first = completed(&mut snapshots, "state.ts", "update", "completed");
        assert!(patch(&first)
            .unwrap()
            .contains("-const value = 'already dirty';"));
        assert!(patch(&first)
            .unwrap()
            .contains("+const value = 'first edit';"));
        fs::write(
            dir.join("state.ts"),
            "const value = 'second edit';\nconst extra = true;\n",
        )
        .unwrap();
        let second = completed(
            &mut snapshots,
            dir.join("state.ts").to_str().unwrap(),
            "update",
            "completed",
        );
        let second = patch(&second).unwrap();
        assert!(second.contains("-const value = 'first edit';"));
        assert!(!second.contains("already dirty"));
        assert!(second.contains("+const extra = true;"));
        let saved = serde_json::to_string(&first[0]).unwrap();
        assert_eq!(
            serde_json::from_str::<ProviderEvent>(&saved).unwrap(),
            first[0]
        );
        fs::remove_file(dir.join("state.ts")).unwrap();
        let deleted = completed(&mut snapshots, "state.ts", "delete", "completed");
        assert!(patch(&deleted)
            .unwrap()
            .contains("-const value = 'second edit';"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn new_files_and_shell_checkpoints_keep_their_own_changes() {
        let dir = repo("new");
        let mut snapshots = EditSnapshots::capture(&dir).unwrap();
        fs::write(dir.join("café file.ts"), "hello\n").unwrap();
        let added = completed(&mut snapshots, "café file.ts", "add", "completed");
        assert!(patch(&added).unwrap().contains("+hello"));
        fs::write(dir.join("café file.ts"), "shell wrote this\n").unwrap();
        snapshots.complete(
            &json!({"type":"item.completed", "item":{"type":"command_execution"}}),
            &mut [],
        );
        fs::write(dir.join("café file.ts"), "the next edit\n").unwrap();
        let next = completed(&mut snapshots, "café file.ts", "update", "completed");
        assert!(patch(&next).unwrap().contains("-shell wrote this"));
        assert!(!patch(&next).unwrap().contains("hello"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unavailable_and_failed_edits_never_get_fabricated_counts() {
        let dir = repo("unavailable");
        fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(dir.join("ignored.txt"), "private old text").unwrap();
        fs::write(dir.join("binary.bin"), [0, 1, 2]).unwrap();
        fs::write(dir.join("large.txt"), vec![b'a'; MAX_FILE + 1]).unwrap();
        let mut snapshots = EditSnapshots::capture(&dir).unwrap();
        for name in ["ignored.txt", "binary.bin", "large.txt"] {
            fs::write(dir.join(name), "new").unwrap();
            assert_eq!(
                patch(&completed(&mut snapshots, name, "add", "completed")),
                None,
                "{name}"
            );
        }
        assert_eq!(
            patch(&completed(
                &mut snapshots,
                "ignored.txt",
                "update",
                "failed"
            )),
            None
        );
        assert!(snapshots.inside("../outside.txt").is_none());
        fs::remove_dir_all(dir).unwrap();
    }
}
