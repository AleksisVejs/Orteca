// An element pointed at in the preview tab. The full block goes to the agent;
// the screen shows the one-line `tidy` form of it.
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

/** A sent prompt back into what the composer holds: its chips and its own words. */
export function split(prompt: string): { picks: Pick[]; rest: string } {
  const picks = [...prompt.matchAll(BLOCK)].map((m) => ({ label: m[2] ?? "", block: m[0] }));
  return { picks, rest: prompt.replace(BLOCK, "").trim() };
}

/** The prompt as the screen shows it: each picked element as a line, not a page. */
export function tidy(prompt: string): string {
  return prompt.replace(BLOCK, (_, _url, element, change) => `Pointed at ${element}: ${change}`);
}
