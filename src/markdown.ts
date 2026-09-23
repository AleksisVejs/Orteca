// The little markdown agents answer in: paragraphs, headings, bullet and
// numbered lists, fenced code, **bold**, `code` and web links. Parsed to plain data so
// the page builds elements from it; agent text never becomes HTML.
// ponytail: no tables, italics or block quotes; add them when an agent's reply needs them.

export type Span = { kind: "text" | "bold" | "code"; text: string } | { kind: "link"; text: string; href: string };
export type ListItem = { depth: number; marker: string; spans: Span[] };
export type Block =
  | { kind: "p"; lines: Span[][] }
  | { kind: "h"; spans: Span[] }
  | { kind: "list"; items: ListItem[] }
  | { kind: "code"; text: string };

export function inline(text: string): Span[] {
  const spans: Span[] = [];
  let last = 0;
  // A web address, bare or behind [words](...), becomes a link; any other
  // [words](target) keeps its words only. Trailing punctuation stays text.
  for (const m of text.matchAll(/\*\*(.+?)\*\*|`([^`]+)`|\[([^\]]+)\]\(([^)\s]*)\)|(https?:\/\/[^\s<>"]*[^\s<>".,;:!?')\]])/g)) {
    if (m.index > last) spans.push({ kind: "text", text: text.slice(last, m.index) });
    if (m[1] !== undefined) spans.push({ kind: "bold", text: m[1] });
    else if (m[2] !== undefined) spans.push({ kind: "code", text: m[2] });
    else if (m[5] !== undefined) spans.push({ kind: "link", text: m[5], href: m[5] });
    else if (/^https?:\/\//.test(m[4] ?? "")) spans.push({ kind: "link", text: m[3] ?? "", href: m[4] ?? "" });
    else spans.push({ kind: "text", text: m[3] ?? "" });
    last = m.index + m[0].length;
  }
  if (last < text.length) spans.push({ kind: "text", text: text.slice(last) });
  return spans;
}

export function parseMarkdown(source: string): Block[] {
  const blocks: Block[] = [];
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? "";
    if (/^\s*```/.test(line)) {
      const code: string[] = [];
      while (++i < lines.length && !/^\s*```/.test(lines[i] ?? "")) code.push(lines[i] ?? "");
      blocks.push({ kind: "code", text: code.join("\n") });
      continue;
    }
    if (!line.trim()) continue;
    const heading = line.match(/^#{1,6}\s+(.*)$/);
    if (heading) {
      blocks.push({ kind: "h", spans: inline(heading[1] ?? "") });
      continue;
    }
    const prev = blocks[blocks.length - 1];
    const item = line.match(/^(\s*)([-*+]|\d+[.)])\s+(.*)$/);
    if (item) {
      const entry = {
        depth: Math.floor((item[1] ?? "").replace(/\t/g, "  ").length / 2),
        marker: /\d/.test(item[2] ?? "") ? (item[2] ?? "") : "•",
        spans: inline(item[3] ?? ""),
      };
      if (prev?.kind === "list" && !blankBefore(lines, i, true)) prev.items.push(entry);
      else blocks.push({ kind: "list", items: [entry] });
      continue;
    }
    // An indented line right under a list item carries that item on.
    if (prev?.kind === "list" && /^\s/.test(line) && !blankBefore(lines, i, false)) {
      prev.items[prev.items.length - 1]?.spans.push({ kind: "text", text: " " }, ...inline(line.trim()));
      continue;
    }
    if (prev?.kind === "p" && !blankBefore(lines, i, false)) prev.lines.push(inline(line));
    else blocks.push({ kind: "p", lines: [inline(line)] });
  }
  return blocks;
}

/** Whether a blank line separates line `i` from the block above. A list item
 *  may skip blanks back to an earlier item; the list stays one list. */
function blankBefore(lines: string[], i: number, listItem: boolean): boolean {
  let j = i - 1;
  while (j >= 0 && !(lines[j] ?? "").trim()) j--;
  if (j === i - 1) return false;
  return !(listItem && /^\s*([-*+]|\d+[.)])\s+/.test(lines[j] ?? ""));
}
