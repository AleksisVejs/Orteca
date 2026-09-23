// An element pointed at in the preview tab, or an earlier task. The full block
// goes to the agent; the screen shows the one-line `tidy` form of it.
export interface Pick {
  /** What the chip says: the tag, its text, the file. */
  label: string;
  block: string;
}

export const CLOSING =
  "Change only this element, not look-alikes elsewhere. If you change its text and it is a translation string, update it in every language file.";

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const BLOCK = new RegExp(
  String.raw`On (\S+), this element: (<[^>]+>(?: ".*")?)\n[\s\S]*?Change: (.*)\n${escape(CLOSING)}`,
  "g",
);

// An earlier task handed to a new one: what was asked, every reply, the final
// answer and the files. Its words are already `tidy`, so one never nests another.
// It says "attached" because that is what the user calls it: headed "for
// context", a model read "this attached task" as the first file Orteca matched.
// The older heading is still read, so tasks saved with it show as one line.
const REFERENCE_END = "End of the earlier task.";
const REFERENCE = new RegExp(
  String.raw`(?:The user attached earlier task #(\d+) from this project|Earlier task #(\d+) in this project, for context): (.*)\n[\s\S]*?\n${escape(REFERENCE_END)}`,
  "g",
);
const refId = (m: RegExpMatchArray | string[]) => m[1] ?? m[2];

export function reference(id: number, title: string, said: string[], answer: string, files: string[]): Pick {
  const lines = [
    `The user attached earlier task #${id} from this project: ${title}`,
    ...said.map((s, i) => `${i === 0 ? "Asked" : "Then"}: ${s}`),
    `Final answer:\n${answer}`,
    ...(files.length ? [`Files it changed: ${files.join(", ")}`] : []),
    REFERENCE_END,
  ];
  return { label: `#${id} ${title}`, block: lines.join("\n") };
}

/** The words a route is chosen from: each earlier task cut to its title. The
 *  whole chat would make the classifier answer about that task instead; none of
 *  it leaves "summarize the attached task" with no words to find files by. */
export const forRouting = (prompt: string) =>
  prompt.replace(REFERENCE, (...m: string[]) => `Attached earlier task #${refId(m)}: ${m[3]}`).trim();

/** A sent prompt back into what the composer holds: its chips and its own words. */
export function split(prompt: string): { picks: Pick[]; rest: string } {
  const picks = [
    ...[...prompt.matchAll(BLOCK)].map((m) => ({ label: m[2] ?? "", block: m[0] })),
    ...[...prompt.matchAll(REFERENCE)].map((m) => ({ label: `#${refId(m)} ${m[3]}`, block: m[0] })),
  ];
  return { picks, rest: prompt.replace(BLOCK, "").replace(REFERENCE, "").trim() };
}

/** The prompt as the screen shows it: each picked element or task as a line, not a page. */
export function tidy(prompt: string): string {
  return prompt
    .replace(BLOCK, (_, _url, element, change) => `Pointed at ${element}: ${change}`)
    .replace(REFERENCE, (...m: string[]) => `Building on task #${refId(m)}: ${m[3]}`);
}
