// Free: did Orteca's Reviews find anything, and when? Reads every saved run
// (out/*.json) of both benchmarks. The question it answers: would "skip the
// Review after a first-try pass on a small change" have lost a finding?
// node scripts/bench/reviews.mjs [bench-dir...]
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const dirs = process.argv.slice(2).length ? process.argv.slice(2)
  : ["riginspect-bench", "liftme-bench"].map((d) => join("C:\\Users\\User\\Projects", d, "out"));

function reviewFacts(run) {
  const stages = run.stages ?? [];
  const reviews = stages.filter((s) => s.stage === "review");
  const firstVerify = stages.find((s) => s.stage === "verify")?.artifact?.verdict ?? null;
  const verifyBeforeReview = stages.findIndex((s) => s.stage === "verify") < stages.findIndex((s) => s.stage === "review");
  // Code only: a test file beside the change says nothing about its risk.
  const code = (run.diff ?? []).filter((d) => d.origin !== "beforeRun" && !/^tests?\//.test(d.path));
  const lines = code.reduce((n, d) => n + (d.added ?? 0) + (d.deleted ?? 0), 0);
  const findings = reviews.flatMap((s) => s.artifact?.findings ?? []);
  return {
    route: run.route?.kind, files: code.length, lines, firstVerify,
    reviews: reviews.length, found: reviews.some((s) => s.artifact?.verdict === "changes_requested"),
    severities: findings.map((f) => f.severity).join(","), issues: findings.map((f) => `${f.severity}: ${f.issue}`),
    // The rule under test: small, not guarded, and the tests passed before the Review read it.
    skippable: run.route?.kind !== "guarded" && code.length <= 1 && lines <= 30 && firstVerify === "pass" && verifyBeforeReview,
  };
}

const rows = [];
for (const dir of dirs.filter(existsSync)) {
  for (const f of readdirSync(dir).filter((f) => f.endsWith(".json"))) {
    try { rows.push({ file: join(dir.split(/[\\/]/).at(-2), f), ...reviewFacts(JSON.parse(readFileSync(join(dir, f), "utf8"))) }); } catch {}
  }
}
const reviewed = rows.filter((r) => r.reviews);
for (const r of reviewed) {
  console.log([r.file.padEnd(46), String(r.route).padEnd(9), `${r.files}f/${r.lines}l`.padEnd(9), `verify:${r.firstVerify ?? "-"}`.padEnd(13),
    r.found ? `FOUND ${r.severities}` : "nothing", r.skippable ? "(skippable)" : ""].join(" "));
  for (const i of r.found ? r.issues : []) console.log("    ", i.slice(0, 160));
}
const count = (xs, p) => `${xs.filter(p).length}/${xs.length}`;
const open = reviewed.filter((r) => r.route !== "guarded");
console.log(`\n${rows.length} runs, ${reviewed.length} with a Review. Found something: ${count(reviewed, (r) => r.found)}.`);
console.log(`Not guarded: found ${count(open, (r) => r.found)}; after a first-try test pass ${count(open.filter((r) => r.firstVerify === "pass"), (r) => r.found)}.`);
const skip = reviewed.filter((r) => r.skippable);
console.log(`Rule would skip ${skip.length}; of those, the Review found something in ${count(skip, (r) => r.found)} (high: ${skip.filter((r) => /high/.test(r.severities)).length}).`);
