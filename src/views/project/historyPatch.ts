export type PatchLine = {
  kind: "context" | "added" | "removed" | "hunk" | "note";
  text: string;
  before?: number;
  after?: number;
};

export type PatchFile = {
  path: string;
  previousPath?: string;
  status: "Modified" | "Added" | "Deleted" | "Renamed" | "Copied";
  added: number;
  removed: number;
  binary: boolean;
  lines: PatchLine[];
  notes: string[];
};

// Git quotes unusual paths and writes non-ASCII bytes as octal escapes.
function gitPath(value: string): string {
  if (!value.startsWith('"')) return value;
  const bytes: number[] = [];
  const escapes: Record<string, string> = { a: "\x07", b: "\b", f: "\f", n: "\n", r: "\r", t: "\t", v: "\v" };
  for (const token of value.slice(1, -1).matchAll(/\\([0-7]{1,3}|.)|([^\\]+)/g)) {
    const escape = token[1] ?? "";
    if (/^[0-7]/.test(escape)) bytes.push(parseInt(escape, 8));
    else bytes.push(...new TextEncoder().encode(token[2] ?? escapes[escape] ?? escape));
  }
  return new TextDecoder().decode(new Uint8Array(bytes));
}

export function parseHistoryPatch(patch: string) {
  const files: PatchFile[] = [];
  const notices: string[] = [];
  // Combined merge diffs have several old-line columns. Preserve the original
  // rather than interpreting them as an ordinary two-version comparison.
  if (/^diff --(?:cc|combined) /m.test(patch)) return { files, notices };

  let file: PatchFile | undefined;
  let before = 0;
  let after = 0;
  let inHunk = false;
  let remainingBefore = 0;
  let remainingAfter = 0;
  let incomplete = false;

  function endHunk() {
    incomplete ||= remainingBefore > 0 || remainingAfter > 0;
    remainingBefore = remainingAfter = 0;
  }

  function start(path: string) {
    endHunk();
    file = { path, status: "Modified", added: 0, removed: 0, binary: false, lines: [], notes: [] };
    files.push(file);
    inHunk = false;
    return file;
  }

  for (const line of patch.split("\n")) {
    if (/^(?:… patch truncated|# patch truncated)/.test(line)) {
      incomplete = true;
      continue;
    }
    if (line.startsWith("diff --git ")) {
      const header = line.slice(11);
      const samePath = header.slice(2, (header.length - 1) / 2);
      const paths = header.match(/^("(?:\\.|[^"\\])*"|a\/.*?) ("(?:\\.|[^"\\])*"|b\/.*)$/);
      start(header === `a/${samePath} b/${samePath}` ? samePath
        : paths ? gitPath(paths[2]!).replace(/^b\//, "") : header);
      continue;
    }
    const untracked = line.match(/^# (binary untracked file|untracked file too large to display|untracked file could not be read): (.*)$/);
    if (untracked) {
      const entry = start(untracked[2]!);
      entry.status = "Added";
      entry.binary = untracked[1] === "binary untracked file";
      entry.notes.push(entry.binary ? "Binary file. A text comparison is not available."
        : untracked[1]!.includes("too large") ? "This new file is too large to display."
        : "This new file could not be read.");
      continue;
    }
    if (!file) continue;

    const hunk = line.match(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@(.*)$/);
    if (hunk) {
      endHunk();
      before = Number(hunk[1]);
      after = Number(hunk[3]);
      remainingBefore = Number(hunk[2] ?? 1);
      remainingAfter = Number(hunk[4] ?? 1);
      inHunk = true;
      file.lines.push({ kind: "hunk", text: hunk[5]!.trim(), before, after });
    } else if (inHunk && /^[ +\-]/.test(line)) {
      const kind = line[0] === "+" ? "added" : line[0] === "-" ? "removed" : "context";
      file.lines.push({ kind, text: line.slice(1),
        before: kind === "added" ? undefined : before++,
        after: kind === "removed" ? undefined : after++ });
      if (kind === "added") file.added++;
      if (kind === "removed") file.removed++;
      if (kind !== "added") remainingBefore--;
      if (kind !== "removed") remainingAfter--;
    } else if (line.startsWith("\\ No newline")) {
      file.lines.push({ kind: "note", text: "No newline at end of file" });
    } else if (!inHunk) {
      if (line.startsWith("new file mode ")) file.status = "Added";
      else if (line.startsWith("deleted file mode ")) file.status = "Deleted";
      else if (line.startsWith("--- ") && line !== "--- /dev/null") {
        file.path = gitPath(line.slice(4).replace(/\t$/, "")).replace(/^a\//, "");
      } else if (line.startsWith("+++ ") && line !== "+++ /dev/null") {
        file.path = gitPath(line.slice(4).replace(/\t$/, "")).replace(/^b\//, "");
      } else if (/^(rename|copy) from /.test(line)) {
        file.previousPath = gitPath(line.replace(/^(rename|copy) from /, ""));
      } else if (/^(rename|copy) to /.test(line)) {
        file.status = line.startsWith("rename") ? "Renamed" : "Copied";
        file.path = gitPath(line.replace(/^(rename|copy) to /, ""));
      } else if (line === "GIT binary patch" || /^Binary files .* differ$/.test(line)) {
        file.binary = true;
        file.notes.push("Binary file. A text comparison is not available.");
      } else if (/^(old|new) mode /.test(line)) {
        file.notes.push(line.replace("old mode", "Previous file mode:").replace("new mode", "New file mode:"));
      }
    }
  }
  endHunk();
  if (incomplete || (patch.length > 0 && !patch.endsWith("\n"))) {
    notices.push("This patch is incomplete. File and line counts include only the changes shown.");
  }
  return { files, notices };
}
