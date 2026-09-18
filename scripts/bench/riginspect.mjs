// Spends real usage. RigInspectBE: Orteca+Claude / Claude / Orteca+Codex / Codex
// on easy, medium, tough, graded by hidden/ tests the agents never see.
// Each arm works in a detached git worktree of RigInspectBE with vendor/ copied
// and node_modules/ junctioned from the real repo.
// node scripts/bench/riginspect.mjs [task...]
//   BENCH=liftme                      LiftMe instead of RigInspectBE (tasks in liftme.tasks.mjs)
//   ARMS=orteca-claude,orteca-codex   which arms (default: all four)
//   RESULTS=results.json              file under riginspect-bench/; finished rows are skipped
// Free modes: SUITE=1 (composer test on HEAD), DRY=1|<task>, REGRADE, DIAG.
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, cpSync, existsSync, lstatSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const LIFTME = process.env.BENCH === "liftme";
const RIG = LIFTME ? "C:\\Users\\User\\Projects\\LiftMe" : "C:\\Users\\User\\Projects\\RigInspectBE";
const ROOT = LIFTME ? "C:\\Users\\User\\Projects\\liftme-bench" : "C:\\Users\\User\\Projects\\riginspect-bench";
const TAURI = join(here, "..", "..", "src-tauri");
const RESULTS = join(ROOT, process.env.RESULTS ?? "results.json");
const ENV = { ...process.env, PATH: `C:\\Program Files\\MySQL\\MySQL Server 9.4\\bin;${process.env.PATH}` };
const CAPS = { "claude week (all models)": 98, "codex week": 90 };
const CAP_DEFAULT = 95;
const ARM_TIMEOUT = 45 * 60_000;
// Mid tier, not the top one: Sonnet for Claude, Terra medium for Codex.
const CLAUDE_ARGS = ["-p", "--output-format", "json", "--permission-mode", "acceptEdits", "--model", "sonnet"];
const CODEX_MODEL = "gpt-5.6-terra";
const CODEX_ARGS = ["exec", "--json", "--sandbox", "workspace-write", "-m", CODEX_MODEL, "-c", "model_reasoning_effort=medium", "-"];

const TASKS = LIFTME ? (await import("./liftme.tasks.mjs")).TASKS : {
  easy: {
    prompt: "GET /api/equipment (EquipmentController@show) returns the entire inventory when per_page is missing or 0. Make it always paginate: missing or 0 means 50 per page, and keep the 100 cap. Update any existing tests that relied on the unpaginated list.",
    hidden: ["BenchEasyPaginationTest.php"],
    also: ["tests/Feature/ExpiredTierTest.php", "tests/Feature/SoftDeleteAuditTest.php"],
  },
  medium: {
    prompt: "GlobalSearchService::searchEquipment has an N+1: it runs a Client query for every assigned client inside the results loop, and it eager-loads each hit's entire checkup history. Make the query count independent of the number of results and load only the checkup-history columns the list needs (id, equipment_id, checkup_id, checkup_date). Keep the response shape unchanged.",
    hidden: ["BenchMediumSearchQueriesTest.php"],
    also: ["tests/Feature/GlobalSearchTest.php"],
  },
  tough: {
    prompt: "The offline sync bootstraps (GET /api/admin/sync/bootstrap and GET /api/technician/sync/bootstrap) re-download the whole inventory and client list every time. Add delta sync: accept an optional since query parameter (ISO 8601). With since, return only equipment and clients whose updated_at is after it, plus deleted_equipment_ids and deleted_client_ids for rows soft-deleted after it, and full: false. An invalid since gets a 422. Without since the response stays a full snapshot with full: true. Share the logic between both controllers instead of duplicating it. Then make resources/js/src/offline/sync.js send since (the previous response's synced_at) when this owner already has a cache, and merge the delta into the cache instead of replacing it. Add backend tests and keep the vitest suite passing.",
    hidden: ["BenchToughDeltaSyncTest.php"],
    also: ["tests/Feature/OfflineCheckupTest.php"],
    vitest: true,
    grep: [["resources/js/src/offline/sync.js", /since/]],
  },
};
const ARMS = (process.env.ARMS ?? "orteca-claude,claude,orteca-codex,codex").split(",");

const git = (...a) => execFileSync("git", a, { cwd: RIG, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
// vendor/ is copied whole. Junctioned packages made ParaTest's workers load the
// real repo's autoloader next to the copy's ("Cannot redeclare class
// ComposerAutoloaderInit..."), so `composer test` could never pass (2026-09-16).
// node_modules stays junctioned; only the tough task's vitest uses it.
const junctions = (dir) => [join(dir, "node_modules")];

function makeWorktree(dir) {
  if (existsSync(dir)) dropWorktree(dir);
  mkdirSync(dirname(dir), { recursive: true });
  git("worktree", "add", "--detach", dir, "HEAD");
  // robocopy exits 0-7 on success; 8+ is a failure.
  const copy = spawnSync("robocopy", [join(RIG, "vendor"), join(dir, "vendor"), "/E", "/MT:16", "/NFL", "/NDL", "/NJH", "/NJS", "/NP"], { stdio: "ignore" });
  if (copy.status === null || copy.status >= 8) throw Error(`robocopy vendor failed: ${copy.status}`);
  for (const j of junctions(dir)) execFileSync("cmd", ["/c", "mklink", "/J", j, j.replace(dir, RIG)], { stdio: "ignore" });
  copyFileSync(join(RIG, ".env"), join(dir, ".env"));
  // Guard against grading the real repo's code again.
  const loaded = execFileSync("php", ["-r", "require 'vendor/autoload.php'; echo (new ReflectionClass('App\\Http\\Controllers\\Controller'))->getFileName();"], { cwd: dir, encoding: "utf8" });
  if (!loaded.toLowerCase().startsWith(dir.toLowerCase())) throw Error(`app classes load from ${loaded}, not ${dir}`);
}

function dropWorktree(dir) {
  // rmdir on a junction removes the link only. Never recurse through it.
  // Found by what is on disk, not by the list, so an older layout is safe too.
  const found = () => [join(dir, "node_modules"), ...(existsSync(join(dir, "vendor")) ? readdirSync(join(dir, "vendor")).map((e) => join(dir, "vendor", e)) : [])]
    .filter((p) => { try { return lstatSync(p).isSymbolicLink(); } catch { return false; } });
  for (const j of found()) execFileSync("cmd", ["/c", "rmdir", j], { stdio: "ignore" });
  if (found().length) throw Error(`junctions still in ${dir}; not removing it`);
  if (!existsSync(join(RIG, "vendor", "autoload.php"))) throw Error("RigInspectBE vendor/ missing - stop");
  git("worktree", "remove", "--force", dir);
}

function limits() {
  const r = spawnSync("cargo", ["test", "bench_limits", "--", "--ignored", "--nocapture"], { cwd: TAURI, encoding: "utf8", shell: true, env: ENV });
  const out = {};
  for (const line of r.stdout.split("\n")) {
    if (!line.startsWith("LIMITS ")) continue;
    const l = JSON.parse(line.slice(7));
    for (const w of l.windows) out[`${l.id} ${w.label}`] = w.usedPercent;
  }
  return out;
}

let terra = null;
async function loadPrice() {
  try {
    const j = await (await fetch("https://models.dev/api.json")).json();
    terra = j.openai.models[CODEX_MODEL]?.cost ?? null;
  } catch {}
}

function runArm(arm, dir, prompt) {
  const t0 = Date.now();
  if (arm.startsWith("orteca-")) {
    const out = join(ROOT, "out", `${arm}-${Date.now()}.json`);
    mkdirSync(dirname(out), { recursive: true });
    const env = { ...ENV, BENCH_DIR: dir, BENCH_PROMPT: prompt, BENCH_PROVIDER: arm.slice(7), BENCH_MODE: "efficient", BENCH_OUT: out };
    const r = spawnSync("cargo", ["test", "bench_run", "--", "--ignored"], { cwd: TAURI, env, encoding: "utf8", shell: true, timeout: ARM_TIMEOUT });
    if (!existsSync(out)) return { status: "noResult", error: (r.stdout + r.stderr).slice(-400), ms: Date.now() - t0 };
    const x = JSON.parse(readFileSync(out, "utf8"));
    const u = x.usage ?? {};
    return {
      status: x.status, route: x.route?.kind, stages: x.stages?.map((s) => s.stage).join(">"), calls: x.callsUsed,
      budgetStop: x.budgetStop?.message, model: u.model, input: u.inputTokens, cached: u.cachedInputTokens, output: u.outputTokens,
      cost: u.costUsd, costQuality: u.costQuality, ms: x.durationMs,
    };
  }
  if (arm === "claude") {
    const r = spawnSync("claude", CLAUDE_ARGS, { cwd: dir, input: prompt, encoding: "utf8", shell: true, env: ENV, timeout: ARM_TIMEOUT, maxBuffer: 64e6 });
    let j;
    try { j = JSON.parse(r.stdout.trim().split("\n").pop()); } catch { return { status: "noResult", error: (r.stdout + r.stderr).slice(-400), ms: Date.now() - t0 }; }
    const u = j.usage ?? {};
    return {
      status: j.subtype, calls: 1, turns: j.num_turns, model: Object.keys(j.modelUsage ?? {}).join(","),
      input: (u.input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0), cached: u.cache_read_input_tokens, output: u.output_tokens,
      cost: j.total_cost_usd, costQuality: "exact", ms: Date.now() - t0,
    };
  }
  const r = spawnSync("codex", CODEX_ARGS, { cwd: dir, input: prompt, encoding: "utf8", shell: true, env: ENV, timeout: ARM_TIMEOUT, maxBuffer: 64e6 });
  let input = 0, cached = 0, output = 0, status = "noTurn";
  for (const line of (r.stdout ?? "").split("\n")) {
    try {
      const v = JSON.parse(line);
      if (v.type === "turn.completed") { status = "done"; input += v.usage.input_tokens ?? 0; cached += v.usage.cached_input_tokens ?? 0; output += v.usage.output_tokens ?? 0; }
      if (v.type === "turn.failed" || v.type === "error") status = "failed";
    } catch {}
  }
  input -= cached; // Codex input includes cached; report uncached like the others.
  const cost = terra ? (input * terra.input + cached * terra.cache_read + output * terra.output) / 1e6 : null;
  return { status, calls: 1, model: CODEX_MODEL, input, cached, output, cost, costQuality: cost == null ? "unavailable" : "estimated", ms: Date.now() - t0 };
}

function phpunit(dir, file) {
  const r = spawnSync("php", ["vendor/bin/phpunit", file], { cwd: dir, encoding: "utf8", env: ENV, timeout: 20 * 60_000 });
  if (r.status === 0) return "pass";
  const plain = (r.stdout ?? "").replace(/\x1b\[[0-9;]*m/g, "");
  const detail = plain.match(/\d\) [^\n]+\n[^\n]+\n[^\n]*/)?.[0] ?? (plain + (r.stderr ?? "") + (r.error?.message ?? "")).slice(-600);
  return "FAIL " + (plain.match(/Tests: .*/)?.[0] ?? `exit ${r.status}`) + " " + detail.replace(/\s+/g, " ");
}

function grade(dir, task, name, arm) {
  // The patch is saved before the hidden tests go in, so it is only the agent's work.
  git("-C", dir, "add", "-A");
  const patch = git("-C", dir, "diff", "--cached", "HEAD");
  mkdirSync(join(ROOT, "patches"), { recursive: true });
  writeFileSync(join(ROOT, "patches", `${name}-${arm}.patch`), patch);
  const stat = git("-C", dir, "diff", "--cached", "--shortstat", "HEAD").trim();
  const checks = {};
  for (const h of task.hidden) {
    const file = `tests/Feature/${h.split("/").pop()}`;
    copyFileSync(join(here, "hidden", h), join(dir, file));
    checks[h] = phpunit(dir, file);
  }
  for (const f of task.also) checks[f] = phpunit(dir, f);
  if (task.vitest) {
    const r = spawnSync("npx", ["vitest", "run"], { cwd: dir, encoding: "utf8", shell: true, env: ENV, timeout: 10 * 60_000 });
    checks.vitest = r.status === 0 ? "pass" : "FAIL " + ((r.stdout ?? "").replace(/\x1b\[[0-9;]*m/g, "").match(/Tests\s+.*/)?.[0] ?? "");
  }
  // Only the agent's added lines count, so text already on HEAD can't pass a check.
  for (const [f, re] of task.grep ?? []) {
    const added = git("-C", dir, "diff", "--cached", "HEAD", "--", f).split("\n").filter((l) => l.startsWith("+")).join("\n");
    checks[`grep ${f}`] = re.test(added) ? "pass" : "FAIL";
  }
  const passed = Object.values(checks).filter((c) => c === "pass").length;
  return { grade: `${passed}/${Object.keys(checks).length}`, checks, diff: stat };
}

if (process.env.REGRADE) {
  // Free: run one extra hidden test against saved patches (and HEAD as control).
  // REGRADE=medium EXTRA=BenchMediumTenantTest.php
  const name = process.env.REGRADE, extra = process.env.EXTRA;
  for (const arm of ["head", ...ARMS]) {
    const patch = join(ROOT, "patches", `${name}-${arm}.patch`);
    if (arm !== "head" && !existsSync(patch)) continue;
    const dir = join(ROOT, "dry", `regrade-${arm}`);
    makeWorktree(dir);
    if (arm !== "head" && readFileSync(patch, "utf8").trim()) execFileSync("git", ["-C", dir, "apply", "--whitespace=nowarn", patch]);
    // A tests/ path is the repo's own (patched) file, run twice to spot a flake.
    if (extra.startsWith("tests/")) {
      console.log(name, arm, extra, phpunit(dir, extra), "| again:", phpunit(dir, extra));
    } else {
      const file = `tests/Feature/${extra.split("/").pop()}`;
      copyFileSync(join(here, "hidden", extra), join(dir, file));
      console.log(name, arm, extra, phpunit(dir, file));
    }
    dropWorktree(dir);
  }
  process.exit(0);
}
if (process.env.DIAG) {
  // Free: the tools agents actually run, in the benchmark's worktree layout.
  const dir = join(ROOT, "dry", "diag");
  makeWorktree(dir);
  const tail = (r) => `exit ${r.status} :: ` + ((r.stdout ?? "") + (r.stderr ?? "")).replace(/\x1b\[[0-9;]*m/g, "").slice(-500).replace(/\s+/g, " ");
  console.log("phpstan", tail(spawnSync("php", ["vendor/bin/phpstan", "analyse", "app/Services/GlobalSearchService.php", "--memory-limit=1G", "--no-progress"], { cwd: dir, encoding: "utf8", env: ENV })));
  // Same argv Orteca's verify_locally builds; TOML single quotes survive cmd.
  console.log("codex sandbox npm test", tail(spawnSync("codex", ["sandbox", "-c", "windows.sandbox='elevated'", "--", "npm.cmd", "test"], { cwd: dir, encoding: "utf8", env: ENV, shell: true, timeout: 300_000 })));
  dropWorktree(dir);
  process.exit(0);
}
if (process.env.SUITE) {
  // Free: Orteca's local Verify command must pass on HEAD in the benchmark layout.
  // SUITE=1 times `composer test`; otherwise ";;"-separated commands, timed in turn.
  const dir = join(ROOT, "dry", "suite");
  makeWorktree(dir);
  for (const command of process.env.SUITE === "1" ? ["composer test"] : process.env.SUITE.split(";;")) {
    const t0 = Date.now();
    const r = spawnSync(command, { cwd: dir, encoding: "utf8", env: ENV, shell: true, timeout: 20 * 60_000 });
    console.log(`${command} :: exit ${r.status} in ${Math.round((Date.now() - t0) / 1000)}s ::`, ((r.stdout ?? "") + (r.stderr ?? "")).replace(/\x1b\[[0-9;]*m/g, "").slice(-300).replace(/\s+/g, " "));
  }
  dropWorktree(dir);
  process.exit(0);
}
if (process.env.DRY) {
  // Free: HEAD must fail each hidden test and pass the existing ones.
  for (const [name, task] of Object.entries(TASKS)) {
    if (process.env.DRY !== "1" && process.env.DRY !== name) continue;
    const dir = join(ROOT, "dry", name);
    makeWorktree(dir);
    console.log(name, JSON.stringify(grade(dir, task, name, "dry"), null, 1));
    if (name === "easy" && !LIFTME) {
      // Positive control: a correct fix must pass, or the grader is broken.
      const f = join(dir, "app", "Http", "Controllers", "EquipmentController.php");
      writeFileSync(f, readFileSync(f, "utf8")
        .replace("$perPage = min(max($perPage, 0), 100);", "$perPage = $perPage > 0 ? min($perPage, 100) : 50;")
        .replaceAll("$perPage > 0 ? $query->paginate($perPage) : $query->get()", "$query->paginate($perPage)")
        .replaceAll("$perPage > 0 ? $equipment->getCollection() : $equipment", "$equipment->getCollection()"));
      console.log("easy control", JSON.stringify(grade(dir, task, name, "control"), null, 1));
    }
    dropWorktree(dir);
  }
  process.exit(0);
}

await loadPrice();
const results = existsSync(RESULTS) ? JSON.parse(readFileSync(RESULTS, "utf8")) : [];
const start = limits();
console.log("limits at start", start);
for (const name of process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(TASKS)) {
  for (const arm of ARMS) {
    if (results.some((r) => r.task === name && r.arm === arm)) continue;
    const dir = join(ROOT, "wt", `${name}-${arm}`);
    makeWorktree(dir);
    console.log(`running ${name} / ${arm}`);
    const m = runArm(arm, dir, TASKS[name].prompt);
    const row = { task: name, arm, ...grade(dir, TASKS[name], name, arm), ...m };
    dropWorktree(dir);
    console.log(JSON.stringify(row, null, 1));
    results.push(row);
    writeFileSync(RESULTS, JSON.stringify(results, null, 2));
    const now = limits();
    console.log("limits", now);
    // Only the providers this run uses can stop it.
    const used = new Set(ARMS.map((a) => a.replace("orteca-", "")));
    const over = Object.entries(now).find(([k, v]) => used.has(k.split(" ")[0]) && v >= (CAPS[k] ?? CAP_DEFAULT));
    if (over) { console.log(`STOP: ${over[0]} at ${over[1]}% (started ${start[over[0]]}%)`); process.exit(2); }
  }
}
console.log("done");
