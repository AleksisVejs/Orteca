// Spends real usage. Does pointing at an element beat describing it?
// Same UI change on LiftMe, only the way the element is named differs:
//   vague, described, screenshot, picked-search (Orteca's reference block, repo
//   search only), picked (the block plus the dev-build source-file line).
// Graded from the diff alone: right line changed, look-alike untouched.
// node scripts/bench/pick.mjs [task...]
//   ARMS=vague,described,...   REPS=2   RESULTS=pick.json (under liftme-bench/)
//   DRY=1     print every prompt, spend nothing
//   REPORT=1  table of results
// The screenshot arm needs liftme-bench/shots/<task>.png (skipped when absent).
// The block's selector and html are written by hand from the page's markup;
// "Likely in the code" is the real find_lines, run on the arm's own worktree.
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const TAURI = join(here, "..", "..", "src-tauri");
const RIG = process.env.RIG ?? "C:\\Users\\User\\Projects\\LiftMe";
const ROOT = join(dirname(RIG), "liftme-bench");
const RESULTS = join(ROOT, process.env.RESULTS ?? "pick.json");
const ARMS = (process.env.ARMS ?? "vague,described,screenshot,picked-search,picked").split(",");
const REPS = Number(process.env.REPS ?? 2);
const CLAUDE_ARGS = ["-p", "--output-format", "json", "--permission-mode", "acceptEdits", "--model", "sonnet"];
const PAGES = "resources/js/pages/";

// hit: an added line in `file` matching every regex. decoys: files that must stay
// untouched; decoyLine: an added line in `file` that means the twin was edited.
const TASKS = {
  "quote-accept": {
    say: "Make the 'Accept quote' button use the site's main action colour instead of green.",
    where: "It's on a signed-in customer's quote page (/account/quotes/12).",
    file: `${PAGES}account/QuoteDetail.vue`,
    hit: [/bg-action/, /@click="accept"/],
    decoys: [`${PAGES}account/QuoteAccess.vue`],
    pick: {
      url: "http://localhost:8000/account/quotes/12", tag: "button", text: "Accept quote",
      selector: "div#app > main > div > div:nth-of-type(3) > div > button:nth-of-type(3)",
      cls: "px-5 py-2.5 rounded-lg bg-success text-white font-semibold hover:opacity-90",
      change: "use the site's main action colour instead of green",
    },
  },
  "login-submit": {
    say: "Make the 'Log in' button fully rounded, a pill, instead of rounded-lg.",
    where: "It's on the login page (/login).",
    file: `${PAGES}Login.vue`,
    hit: [/rounded-full/],
    decoys: [`${PAGES}Register.vue`],
    pick: {
      url: "http://localhost:8000/login", tag: "button", text: "Log in",
      selector: "div#app > main > div > form > button",
      cls: "w-full py-2.5 rounded-lg bg-action text-white font-semibold hover:bg-action-hover disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-action/50",
      change: "make it fully rounded, a pill, instead of rounded-lg",
    },
  },
  "checkout-pay": {
    say: "Give the 'Pay' button more vertical padding: py-4 instead of py-3.",
    where: "It's on the checkout page (/checkout), on the step where the card form shows.",
    file: `${PAGES}Checkout.vue`,
    hit: [/py-4/, /@click="pay"/],
    decoyLine: /@click="startPayment"/,
    pick: {
      url: "http://localhost:8000/checkout", tag: "button", text: "Pay €120.00",
      selector: "div#app > main > div > div:nth-of-type(2) > button",
      cls: "mt-4 w-full py-3 rounded-lg bg-action text-white font-semibold hover:bg-action-hover disabled:opacity-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-action/50",
      change: "more vertical padding: py-4 instead of py-3",
    },
  },
  "messages-send": {
    say: "Make the 'Send' button's text base size, text-base instead of text-sm.",
    where: "It's on the account Messages page (/account/messages), in the new-message form.",
    file: `${PAGES}account/Messages.vue`,
    hit: [/text-base/, /startNew/],
    decoyLine: /starting = !starting/,
    pick: {
      url: "http://localhost:8000/account/messages", tag: "button", text: "Send",
      selector: "div#app > main > div > div:nth-of-type(2) > div > button",
      cls: "px-4 py-2 rounded-lg bg-action text-white text-sm font-medium hover:bg-action-hover focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-action/50",
      change: "make its text base size, text-base instead of text-sm",
    },
  },
};

const git = (dir, ...a) => execFileSync("git", ["-C", dir, ...a], { encoding: "utf8", maxBuffer: 256e6 });
const slash = (p) => p.replaceAll("\\", "/");

function found(dir, needles) {
  const r = spawnSync("cargo", ["test", "bench_find", "--", "--ignored", "--nocapture"], {
    cwd: TAURI, encoding: "utf8", shell: true, env: { ...process.env, BENCH_DIR: dir, BENCH_NEEDLES: JSON.stringify(needles) },
  });
  const line = r.stdout.split("\n").find((l) => l.startsWith("FOUND "));
  if (!line) throw Error("bench_find printed nothing: " + (r.stdout + r.stderr).slice(-300));
  return JSON.parse(line.slice(6));
}

// Same shape DockPreview.vue writes into the prompt box.
function block(t, dir, withFile) {
  const p = t.pick;
  const html = `<${p.tag} class="${p.cls}">${p.text}</${p.tag}>`;
  const likely = found(dir, [p.text, p.cls]);
  return [
    `On ${p.url}, this element: <${p.tag}> "${p.text}"`,
    ...(withFile ? [`Source file: ${slash(join(dir, t.file))}`] : []),
    `Selector: ${p.selector}`,
    `HTML: ${html}`,
    ...(likely.length ? ["Likely in the code (best first):", ...likely.map((l) => `  ${l}`)] : []),
    `Change: ${p.change}`,
    // Same as CLOSING in src/views/project/picks.ts.
    "Change only this element, not look-alikes elsewhere. If you change its text and it is a translation string, update it in every language file.",
  ].join("\n");
}

function prompt(arm, name, t, dir) {
  const site = "In the LiftMe site (Laravel + Vue, this repo)";
  switch (arm) {
    case "vague": return `${site}: ${t.say}`;
    case "described": return `${site}: ${t.say} ${t.where}`;
    case "screenshot": return `${site}: ${t.say} A screenshot of the page: ${slash(join(ROOT, "shots", name + ".png"))}`;
    case "picked-search": return block(t, dir, false);
    case "picked": return block(t, dir, true);
  }
}

function grade(dir, t) {
  git(dir, "add", "-A");
  const files = git(dir, "diff", "--cached", "--name-only", "HEAD").split("\n").filter(Boolean);
  const added = (f) => git(dir, "diff", "--cached", "-U0", "HEAD", "--", f).split("\n").filter((l) => l.startsWith("+") && !l.startsWith("+++"));
  const hit = added(t.file).some((l) => t.hit.every((re) => re.test(l)));
  const wrong = (t.decoys ?? []).some((f) => files.includes(f)) || (t.decoyLine ? added(t.file).some((l) => t.decoyLine.test(l)) : false);
  return { result: hit && !wrong ? "exact" : wrong ? "wrong" : "miss", hit, wrong, files, patch: git(dir, "diff", "--cached", "HEAD") };
}

function run(dir, text) {
  const t0 = Date.now();
  const r = spawnSync("claude", CLAUDE_ARGS.concat(["--add-dir", join(ROOT, "shots")]), { cwd: dir, input: text, encoding: "utf8", shell: true, timeout: 15 * 60_000, maxBuffer: 64e6 });
  let j;
  try { j = JSON.parse(r.stdout.trim().split("\n").pop()); } catch { return { status: "noResult", error: (r.stdout + r.stderr).slice(-300), ms: Date.now() - t0 }; }
  const u = j.usage ?? {};
  return {
    status: j.subtype, turns: j.num_turns, cost: j.total_cost_usd,
    input: (u.input_tokens ?? 0) + (u.cache_creation_input_tokens ?? 0), cached: u.cache_read_input_tokens, output: u.output_tokens, ms: Date.now() - t0,
  };
}

function worktree(dir) {
  if (existsSync(dir)) execFileSync("git", ["-C", RIG, "worktree", "remove", "--force", dir]);
  mkdirSync(dirname(dir), { recursive: true });
  execFileSync("git", ["-C", RIG, "worktree", "add", "--detach", dir, "HEAD"], { stdio: "ignore" });
}

const rows = existsSync(RESULTS) ? JSON.parse(readFileSync(RESULTS, "utf8")) : [];

if (process.env.REPORT) {
  const avg = (xs) => (xs.length ? xs.reduce((a, b) => a + b, 0) / xs.length : NaN);
  console.log("arm".padEnd(15), "runs exact wrong miss  $/run  turns  secs");
  for (const arm of ARMS) {
    const r = rows.filter((x) => x.arm === arm);
    const n = (k) => r.filter((x) => x.result === k).length;
    console.log(arm.padEnd(15), String(r.length).padStart(4), String(n("exact")).padStart(5), String(n("wrong")).padStart(5), String(n("miss")).padStart(4),
      avg(r.map((x) => x.cost ?? 0)).toFixed(3).padStart(6), avg(r.map((x) => x.turns ?? 0)).toFixed(1).padStart(6), (avg(r.map((x) => x.ms)) / 1000).toFixed(0).padStart(5));
  }
  process.exit(0);
}

mkdirSync(join(ROOT, "shots"), { recursive: true });
const names = process.argv.slice(2).length ? process.argv.slice(2) : Object.keys(TASKS);
for (const name of names) {
  const t = TASKS[name];
  for (let rep = 1; rep <= REPS; rep++) {
    for (const arm of ARMS) {
      if (rows.some((r) => r.task === name && r.arm === arm && r.rep === rep)) continue;
      if (arm === "screenshot" && !existsSync(join(ROOT, "shots", name + ".png"))) { console.log(`skip ${name}/screenshot: no shots/${name}.png`); continue; }
      const dir = join(ROOT, "pickwt", `${name}-${arm}-${rep}`);
      worktree(dir);
      const text = prompt(arm, name, t, dir);
      if (process.env.DRY) { console.log(`\n=== ${name} / ${arm}\n${text}`); execFileSync("git", ["-C", RIG, "worktree", "remove", "--force", dir]); continue; }
      console.log(`running ${name} / ${arm} / ${rep}`);
      const m = run(dir, text);
      const g = grade(dir, t);
      execFileSync("git", ["-C", RIG, "worktree", "remove", "--force", dir]);
      mkdirSync(join(ROOT, "patches"), { recursive: true });
      writeFileSync(join(ROOT, "patches", `pick-${name}-${arm}-${rep}.patch`), g.patch);
      const { patch, ...rest } = g;
      const row = { task: name, arm, rep, ...rest, ...m };
      console.log(JSON.stringify(row));
      rows.push(row);
      writeFileSync(RESULTS, JSON.stringify(rows, null, 2));
    }
  }
}
console.log("done");
