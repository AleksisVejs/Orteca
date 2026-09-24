//! What the dock's tabs need from the machine: a real ConPTY per terminal, a
//! directory listing for the file tree, and one text file in and out for the
//! code tab. Everything here is gated on a trusted project and confined to it.
//!
//! The PTY child goes in a Job Object for the same reason `proc` does it - a
//! shell spawns `npm -> node -> vite`, and closing a tab has to take the whole
//! tree with it, not just the shell.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::error::{AppError, ErrorKind, Result};
use crate::proc::Job;
use crate::store::Store;

/// Refuse to load something that is not a source file into a text editor.
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;

/// One live terminal. Dropping it closes the job handle, which kills the tree.
struct Terminal {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    job: Job,
}

#[derive(Default)]
pub struct Terminals {
    live: Arc<Mutex<HashMap<u32, Terminal>>>,
    next: AtomicU32,
}

/// One row of the file tree. `dir` decides whether clicking it expands or opens.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    name: String,
    dir: bool,
}

/// Resolve `sub` under the trusted project root, refusing anything that climbs
/// out of it through `..` or a symlink.
fn inside(store: &Store, path: &str, sub: &str) -> Result<(PathBuf, PathBuf)> {
    let (root, _) = crate::trusted_dir(store, path)?;
    let target = root.join(sub.trim_start_matches(['/', '\\']));
    let target = target.canonicalize().map_err(|_| {
        AppError::new(ErrorKind::NotFound, format!("{sub} is not there anymore."))
    })?;
    if !target.starts_with(&root) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "That path is outside this project.",
        ));
    }
    Ok((root, target))
}

/// The shell a terminal opens with. An empty preference means "whatever this
/// machine has": PowerShell 7 when it is installed, else the one Windows ships.
fn shell_command(shell: &str, cwd: &Path) -> Result<CommandBuilder> {
    let words = if shell.trim().is_empty() {
        vec![crate::providers::which("pwsh")
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "powershell.exe".into())]
    } else {
        shell_words::split(shell)
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("That shell command does not parse: {e}")))?
    };
    let (program, args) = words
        .split_first()
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "Type a shell command first."))?;
    let mut command = CommandBuilder::new(program);
    command.args(args);
    command.cwd(cwd);
    // A pager that pauses for a keypress is fine here; a terminal has a keyboard.
    command.env("TERM", "xterm-256color");
    Ok(command)
}

/// Where a terminal opens: the folder itself, or the folder holding the file,
/// and never in the `\\?\` form `canonicalize` returns. No shell works out of
/// that form - cmd refuses it and defaults to the Windows directory, and
/// PowerShell starts on a provider-qualified path where `npm` cannot find the
/// project's own binaries.
fn shell_cwd(target: &Path) -> PathBuf {
    let dir = if target.is_dir() { target } else { target.parent().unwrap_or(target) };
    crate::project::plain(dir)
}

/// Open a shell in `sub` and stream it to the window as `pty:<id>`.
#[tauri::command(async)]
#[allow(clippy::too_many_arguments)]
pub fn pty_open(
    app: AppHandle,
    path: String,
    sub: String,
    shell: String,
    cols: u16,
    rows: u16,
    terminals: State<Terminals>,
    store: State<Store>,
) -> Result<u32> {
    let (_, target) = inside(&store, &path, &sub)?;
    let cwd = shell_cwd(&target);
    let pair = native_pty_system()
        .openpty(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| AppError::new(ErrorKind::Io, format!("No terminal could be opened: {e}")))?;

    let job = Job::new()?;
    let child = pair
        .slave
        .spawn_command(shell_command(&shell, &cwd)?)
        .map_err(|e| AppError::new(ErrorKind::Io, format!("That shell would not start: {e}")))?;
    // The shell is already running, so its own descendants are what the job is
    // really for; adopting it a moment late still catches every one of them.
    if let Some(pid) = child.process_id() {
        let _ = job.assign(pid);
    }
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("The terminal could not be read: {e}")))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("The terminal could not be written to: {e}")))?;

    let id = terminals.next.fetch_add(1, Ordering::Relaxed) + 1;
    terminals
        .live
        .lock()
        .expect("terminals poisoned")
        .insert(id, Terminal { master: pair.master, writer, job });

    let live = Arc::clone(&terminals.live);
    std::thread::spawn(move || {
        pump(app, id, reader);
        live.lock().expect("terminals poisoned").remove(&id);
    });
    Ok(id)
}

/// Take the whole characters off the front of `pending`, leaving any trailing
/// bytes of a character the read stopped in the middle of. Without this a
/// multi-byte character split across two reads becomes a replacement character
/// on screen, and a terminal never repaints what it already printed.
fn take_whole(pending: &mut Vec<u8>) -> Option<String> {
    let good = match std::str::from_utf8(pending) {
        Ok(_) => pending.len(),
        Err(e) => e.valid_up_to(),
    };
    if good == 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&pending[..good]).into_owned();
    pending.drain(..good);
    Some(text)
}

/// Read the shell's output forever, emitting it to the window as it arrives.
///
/// Every emit costs the window's main thread a task, and a shell prints a line
/// at a time: a build's fifty thousand lines that way is fifty thousand tasks,
/// and the window stops answering Windows. Pausing a frame after each emit lets
/// the next read take the whole burst that piled up, so the emits stay at a
/// screen's worth per second however loud the program is.
fn pump(app: AppHandle, id: u32, mut reader: Box<dyn Read + Send>) {
    let mut buffer = [0u8; 65536];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(size) => {
                pending.extend_from_slice(&buffer[..size]);
                let Some(text) = take_whole(&mut pending) else { continue };
                if app.emit(&format!("pty:{id}"), text).is_err() {
                    break;
                }
                // ponytail: a fixed frame, not a measured one. The pty buffers
                // what the shell prints meanwhile; raise it if output lags.
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
        }
    }
    let _ = app.emit(&format!("pty-exit:{id}"), ());
}

// Off the main thread: a shell that stops reading can block this write, and
// the window must not freeze with it.
#[tauri::command(async)]
pub fn pty_write(id: u32, data: String, terminals: State<Terminals>) -> Result<()> {
    let mut live = terminals.live.lock().expect("terminals poisoned");
    let terminal = live
        .get_mut(&id)
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, "That terminal has closed."))?;
    terminal.writer.write_all(data.as_bytes())?;
    terminal.writer.flush()?;
    Ok(())
}

#[tauri::command]
pub fn pty_resize(id: u32, cols: u16, rows: u16, terminals: State<Terminals>) -> Result<()> {
    let live = terminals.live.lock().expect("terminals poisoned");
    // A resize after the shell exited is the tab's layout settling, not an error.
    if let Some(terminal) = live.get(&id) {
        let _ = terminal.master.resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        });
    }
    Ok(())
}

/// Close one terminal and everything it started. Closing an already-closed one
/// is what happens when the shell exited first, so it is not a failure.
#[tauri::command]
pub fn pty_close(id: u32, terminals: State<Terminals>) -> Result<()> {
    if let Some(terminal) = terminals.live.lock().expect("terminals poisoned").remove(&id) {
        let _ = terminal.job.terminate();
    }
    Ok(())
}

/// One directory's children, folders first. `.git` stays hidden - it is the one
/// folder in a repository that nobody opens a file from.
#[tauri::command(async)]
pub fn list_dir(path: String, sub: String, store: State<Store>) -> Result<Vec<Entry>> {
    let (_, dir) = inside(&store, &path, &sub)?;
    let mut entries: Vec<Entry> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name != ".git").then(|| Entry {
                dir: e.file_type().is_ok_and(|t| t.is_dir()),
                name,
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

/// The repository's own history, newest first. Paged rather than capped: the
/// tab asks for more when the reader gets to the end of what it has.
#[tauri::command(async)]
pub fn git_log(path: String, skip: u32, count: u32, store: State<Store>) -> Result<Vec<crate::project::Commit>> {
    let (root, _) = crate::trusted_dir(&store, &path)?;
    crate::project::git_log(&root, skip, count.clamp(1, 200))
}

/// Where the repo mentions what a picked preview element shows.
#[tauri::command(async)]
pub fn find_lines(path: String, needles: Vec<String>, store: State<Store>) -> Result<Vec<String>> {
    let (root, _) = crate::trusted_dir(&store, &path)?;
    Ok(crate::project::find_lines(&root, &needles))
}

/// What one commit changed, as a patch.
#[tauri::command(async)]
pub fn commit_patch(path: String, hash: String, store: State<Store>) -> Result<String> {
    let (root, _) = crate::trusted_dir(&store, &path)?;
    crate::project::commit_patch(&root, &hash)
}

/// What the working tree has that no commit holds yet, as a patch. Untracked
/// files are in it too, which `git diff` alone would leave out.
#[tauri::command(async)]
pub fn working_patch(path: String, store: State<Store>) -> Result<String> {
    let (root, _) = crate::trusted_dir(&store, &path)?;
    crate::project::working_patch(&root)
}

/// One file's text. A binary file is refused rather than rendered as noise.
#[tauri::command(async)]
pub fn read_text(path: String, file: String, store: State<Store>) -> Result<String> {
    let (_, target) = inside(&store, &path, &file)?;
    let size = target.metadata()?.len();
    if size > MAX_TEXT_BYTES {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("That file is {} MB - too big to open here.", size / 1024 / 1024),
        ));
    }
    let bytes = std::fs::read(&target)?;
    String::from_utf8(bytes)
        .map_err(|_| AppError::new(ErrorKind::Invalid, "That file is not text."))
}

/// `base` is the text the tab loaded. A file that no longer holds it was
/// changed by something else - a run, a terminal, another editor - and saving
/// over it would silently throw that change away.
#[tauri::command(async)]
pub fn write_text(path: String, file: String, text: String, base: String, store: State<Store>) -> Result<()> {
    // The file must already exist: the code tab edits, it does not create.
    let (_, target) = inside(&store, &path, &file)?;
    if !target.is_file() {
        return Err(AppError::new(ErrorKind::NotFound, "That file is not there anymore."));
    }
    if std::fs::read(&target)? != base.as_bytes() {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "This file changed on disk since you opened it. Reload to see the new version; your edits stay in the box until you do.",
        ));
    }
    std::fs::write(&target, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_shell_preference_resolves_to_a_real_powershell() {
        let command = shell_command("", Path::new(".")).unwrap();
        let program = command.get_argv()[0].to_string_lossy().to_lowercase();
        assert!(program.ends_with("pwsh.exe") || program == "powershell.exe", "{program}");
    }

    #[test]
    fn a_shell_preference_keeps_its_arguments_and_quoting() {
        let command = shell_command(r#""C:\Program Files\Git\bin\bash.exe" --login -i"#, Path::new(".")).unwrap();
        let argv: Vec<String> = command.get_argv().iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(argv, [r"C:\Program Files\Git\bin\bash.exe", "--login", "-i"]);
    }

    #[test]
    fn an_unparseable_shell_is_refused_before_anything_spawns() {
        assert!(shell_command("\"unclosed", Path::new(".")).is_err());
    }

    #[test]
    fn a_terminal_never_opens_on_the_path_form_a_shell_refuses() {
        // What `inside` hands over: a project folder straight from `canonicalize`.
        let canonical = std::env::current_dir().unwrap().canonicalize().unwrap();
        assert!(canonical.to_string_lossy().starts_with(r"\\?\"), "{canonical:?}");

        let cwd = shell_cwd(&canonical);
        assert!(!cwd.to_string_lossy().starts_with(r"\\?\"), "{cwd:?}");
        assert!(cwd.is_dir());

        // A file opens its folder, still without the prefix.
        let file = canonical.join("Cargo.toml");
        assert_eq!(shell_cwd(&file), cwd);
    }

    #[test]
    fn a_character_split_across_two_reads_is_held_until_it_is_whole() {
        // "ok OK" where OK is a three-byte tick, and this read stops after one byte.
        let mut pending = b"ok \xe2".to_vec();
        assert_eq!(take_whole(&mut pending).as_deref(), Some("ok "));
        assert_eq!(pending, b"\xe2");

        pending.extend_from_slice(b"\x9c\x93 done");
        assert_eq!(take_whole(&mut pending).as_deref(), Some("\u{2713} done"));
        assert!(pending.is_empty());
    }

    #[test]
    fn a_read_that_is_only_half_a_character_emits_nothing_yet() {
        let mut pending = b"\xe2\x9c".to_vec();
        assert!(take_whole(&mut pending).is_none());
        assert_eq!(pending.len(), 2);
    }
}
