// Builds a throwaway repository for exercising Orteca against the real CLIs.
//
// Fixtures cover the parsers; they cannot cover a process that has to be
// cancelled, steered, or made to produce a diff git reports oddly. This repo is
// the other half of that: a real git tree, outside Orteca's own, seeded with the
// cases `project::diff_since` is most likely to get wrong - a path with a space,
// a non-ASCII path git C-quotes, a binary file numstat writes as `-`, an ignored
// file an agent can edit invisibly, and an untracked file that was already there
// before any run. Its tests are slow on purpose, so a run lasts long enough to
// interrupt.
//
// Re-running rebuilds it from scratch, which is the reset. It refuses to touch a
// directory that is not already marked as a sandbox.
//
// Run: node scripts/make-sandbox.mjs [path]

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const MARKER = ".orteca-sandbox";
// Beside Orteca, never inside it: a nested repo would confuse the git-root
// resolution in `open_project`, and an agent let loose here must not be able to
// reach the real source tree by walking up one directory.
const target = resolve(process.argv[2] ?? join(here, "..", "..", "orteca-sandbox"));

if (existsSync(target) && !existsSync(join(target, MARKER))) {
  console.error(`Refusing to rebuild ${target}: no ${MARKER} file, so this is not a sandbox.`);
  console.error("Pass a different path, or delete that directory yourself if you meant it.");
  process.exit(1);
}
rmSync(target, { recursive: true, force: true });

/** Tracked at the baseline commit. The agent's diff is measured against these. */
const TRACKED = {
  [MARKER]: "Rebuild with: node scripts/make-sandbox.mjs\n",

  "README.md": `# Orteca sandbox

Throwaway. Every file here exists to be edited, broken or deleted by an agent
under test. Put nothing real in it - \`node scripts/make-sandbox.mjs\` wipes and
rebuilds the whole directory.

\`npm test\` is slow on purpose (~6s), so a run lasts long enough to cancel.
`,

  // Both filenames, so each CLI reads its own and the trust scan reports both.
  "CLAUDE.md": `# Sandbox

A scratch JavaScript project, no dependencies.

**Run the tests with \`node --test "src/*.test.js"\` - not \`npm test\`.**
npm cannot run here: it resolves its own path through the user profile, which
the sandbox denies, and PowerShell will not load \`npm.ps1\` under this
machine's execution policy. \`node\` itself works fine. An agent that reaches
for npm burns several turns rediscovering that.

\`src/slug.js\` has failing tests. \`src/tally.js\` is deliberately slow, so a
run lasts long enough to interrupt.
`,
  "AGENTS.md": `See CLAUDE.md. No dependencies.

Run the tests with \`node --test "src/*.test.js"\`. Do not use npm: it cannot
run under the sandbox, and working around it costs several turns.
`,

  // Inert on purpose: enough for the trust scan to report a finding, with no
  // hook and no MCP server, so consenting to this repo executes nothing.
  ".claude/settings.json": `{\n  "$comment": "Inert. No hooks - this repo is a trust-scan target, not a payload."\n}\n`,
  ".mcp.json": `{\n  "mcpServers": {}\n}\n`,

  ".gitignore": "build/\n*.log\n",

  "package.json": `{
  "name": "orteca-sandbox",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "node --test src/*.test.js"
  }
}
`,

  // Two of these three tests fail. The bug is real and small: every
  // non-alphanumeric run becomes its own dash, and a trailing one is kept.
  "src/slug.js": `export function slug(title) {
  return title.toLowerCase().replace(/[^a-z0-9]/g, "-");
}
`,
  "src/slug.test.js": `import { test } from "node:test";
import assert from "node:assert/strict";
import { slug } from "./slug.js";

test("lowercases and joins words", () => {
  assert.equal(slug("Hello World"), "hello-world");
});

test("collapses repeated separators", () => {
  assert.equal(slug("Hello   World"), "hello-world");
});

test("drops trailing punctuation", () => {
  assert.equal(slug("Ship it!"), "ship-it");
});
`,

  "src/tally.js": `export function tally(rows) {
  const totals = new Map();
  for (const { key, amount } of rows) {
    totals.set(key, (totals.get(key) ?? 0) + amount);
  }
  return totals;
}
`,
  // Slow so a verification cycle costs real seconds, which is the window a
  // cancel or a mid-run instruction has to land in.
  "src/tally.test.js": `import { test } from "node:test";
import assert from "node:assert/strict";
import { setTimeout as sleep } from "node:timers/promises";
import { tally } from "./tally.js";

const SLOW = 2000;

test("sums by key", async () => {
  await sleep(SLOW);
  assert.equal(tally([{ key: "a", amount: 2 }, { key: "a", amount: 3 }]).get("a"), 5);
});

test("keeps keys apart", async () => {
  await sleep(SLOW);
  assert.equal(tally([{ key: "a", amount: 1 }, { key: "b", amount: 9 }]).get("b"), 9);
});

test("handles nothing at all", async () => {
  await sleep(SLOW);
  assert.equal(tally([]).size, 0);
});
`,

  // Unused. Ask an agent to delete it and watch the denylist refuse `rm`.
  "src/old.js": "// Nothing imports this. It exists to be deleted.\nexport const unused = true;\n",

  // A space in the path: numstat and ls-files disagree about how to print it.
  "notes/design notes.md": "Renaming `slug.js` to `slugify.js` makes numstat emit `old => new` in one field.\n",
  // Non-ASCII: git C-quotes this under the default core.quotePath.
  "notes/caf\u00e9-decisions.md": "Decided over coffee. The filename is the test.\n",
};

function put(relative, contents) {
  const file = join(target, relative);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, contents);
}

for (const [relative, contents] of Object.entries(TRACKED)) put(relative, contents);

// Binary: numstat reports `-` for both counts rather than a line delta.
const blob = Buffer.alloc(512);
for (let i = 0; i < blob.length; i += 1) blob[i] = (i * 7) % 256;
mkdirSync(join(target, "assets"), { recursive: true });
writeFileSync(join(target, "assets", "logo.bin"), blob);

const git = (...args) =>
  execFileSync("git", args, { cwd: target, stdio: ["ignore", "pipe", "pipe"] }).toString().trim();

git("init", "-q", "-b", "main");
// Local identity, so the baseline commit works whatever the global config says.
git("config", "user.name", "Orteca Sandbox");
git("config", "user.email", "sandbox@orteca.invalid");
git("add", "-A");
git("commit", "-qm", "Baseline for Orteca runs");

// Written after the commit, so both are present before any agent starts.
// `build/generated.txt` is ignored: an agent can edit it and the diff must not
// claim it. `stray.txt` is untracked: it must not be counted as the agent's work,
// and it is what makes the project dirty at start.
put("build/generated.txt", "Ignored by .gitignore. An edit here must not show in a run's diff.\n");
put("stray.txt", "Untracked before any run. A run's diff must not claim this file.\n");

console.log(`Sandbox ready: ${target}`);
console.log(`  baseline ${git("rev-parse", "--short", "HEAD")} on ${git("rev-parse", "--abbrev-ref", "HEAD")}`);
console.log(`  dirty at start: stray.txt untracked, build/ ignored`);
