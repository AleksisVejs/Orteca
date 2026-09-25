// Where a Claude run's tokens went, stage by stage, from its recordings.
// node scripts/bench/turns.mjs <recordings dir | task-*.jsonl ...>
// Free: reads files only. A bench row's `record` names its folder; the app's
// own runs are under %APPDATA%/app.orteca/recordings.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, join } from "node:path";

const files = process.argv.slice(2).flatMap((p) =>
  statSync(p).isDirectory() ? readdirSync(p).filter((f) => f.endsWith(".jsonl")).sort().map((f) => join(p, f)) : [p]);
if (!files.length) throw Error("usage: turns.mjs <dir | file.jsonl ...>");

const readBy = new Map(); // file path -> stages that read it
for (const file of files) {
  const lines = readFileSync(file, "utf8").split("\n").filter(Boolean).flatMap((l) => { try { return [JSON.parse(l)]; } catch { return []; } });
  const seen = new Set();
  const t = { turns: 0, input: 0, write5m: 0, write1h: 0, read: 0, output: 0 };
  const tools = {}, results = [], reads = {};
  let result;
  for (const v of lines) {
    if (v.type === "assistant") {
      const u = v.message?.usage;
      if (u && !seen.has(v.message.id)) {
        seen.add(v.message.id);
        t.turns++;
        t.input += u.input_tokens ?? 0;
        t.write5m += u.cache_creation?.ephemeral_5m_input_tokens ?? 0;
        t.write1h += u.cache_creation?.ephemeral_1h_input_tokens ?? (u.cache_creation ? 0 : u.cache_creation_input_tokens ?? 0);
        t.read += u.cache_read_input_tokens ?? 0;
        t.output += u.output_tokens ?? 0;
      }
      for (const b of v.message?.content ?? []) {
        if (b.type !== "tool_use") continue;
        tools[b.name] = (tools[b.name] ?? 0) + 1;
        const path = b.input?.file_path;
        if (b.name === "Read" && path) reads[path] = (reads[path] ?? 0) + 1;
      }
    }
    if (v.type === "user") {
      for (const b of v.message?.content ?? []) {
        if (b.type === "tool_result") results.push((typeof b.content === "string" ? b.content : JSON.stringify(b.content)).length);
      }
    }
    if (v.type === "result") result = v;
  }
  const stage = basename(file, ".jsonl");
  for (const path of Object.keys(reads)) readBy.set(path, [...(readBy.get(path) ?? []), stage]);
  const usage = Object.entries(result?.modelUsage ?? {}).map(([m, u]) => `${m} $${u.costUSD?.toFixed(3)} thinking ${u.thinkingTokens ?? "?"}`).join(", ");
  results.sort((a, b) => b - a);
  console.log(`\n${stage}  $${result?.total_cost_usd?.toFixed(3) ?? "?"}  ${usage}`);
  // A streamed message's usage is taken as it starts, so its output count is
  // partial; the result's own total is the real one.
  const output = result?.usage?.output_tokens ?? t.output;
  console.log(`  turns ${t.turns}  input ${t.input}  cache write 5m ${t.write5m} / 1h ${t.write1h}  cache read ${t.read}  output ${output}`);
  console.log(`  tools ${JSON.stringify(tools)}`);
  console.log(`  tool results ${results.length}, ${results.reduce((a, b) => a + b, 0)} chars; largest ${results.slice(0, 5).join(", ")}`);
  const again = Object.entries(reads).filter(([, n]) => n > 1);
  if (again.length) console.log(`  read more than once: ${again.map(([p, n]) => `${basename(p)} x${n}`).join(", ")}`);
}
const shared = [...readBy].filter(([, stages]) => stages.length > 1);
if (shared.length) console.log(`\nread in more than one stage: ${shared.map(([p, s]) => `${basename(p)} (${s.map((x) => x.split("-")[2]).join(", ")})`).join("; ")}`);
