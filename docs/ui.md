# UI rules

The look takes inspiration from [Orca’s desktop workspace](https://www.onorca.dev/):
neutral black panels, compact navigation, fine dividers and a slim status bar.
It retains the Velo icon, near-white text, green for outcomes and blue for work in
flight. Everything below is a rule, not a suggestion. `src/styles/tokens.css`
is the only place colour, radius and spacing are defined.

## Colour

- **Never hardcode a colour in a component.** Use a token. If a token is
  missing, add it to `tokens.css` — do not invent a one-off hex.
- Four surfaces only: `--bg` (page), `--surface` (card), `--surface-2`
  (raised / hover / selected), `--border` / `--border-strong` (hairlines).
  There is no fifth step. If a layout needs one, the layout is too deep.
- Three text steps only: `--text`, `--text-dim`, `--text-faint`. Body copy is
  `--text-dim`; labels and metadata are `--text-faint`.
- **Green (`--accent`) means an outcome that actually happened** — a run
  finished, a real cost was measured, a saving was computed from a baseline.
  Never use green for decoration, for a heading, or for a number the backend
  could not verify.
- **Blue (`--info`) means in flight, and only that** — the progress bar while a
  run is streaming. It is never a link colour, never a button fill.
- `--warn` is for consent, risk and missing prerequisites (untrusted project,
  CLI not installed, uncommitted changes). `--err` is for a failure that
  already happened.
- Backgrounds are flat. No gradients, no glass, no blur. Primary actions use
  `--button` with dark text; neutral `--focus` rings identify keyboard focus.

## The mark

- `src/assets/velo.png` is the only copy of the fox. `VeloMark.vue` renders it
  and `scripts/make-icon.mjs` builds every file in `src-tauri/icons/` from it —
  `icon.ico` plus the PNG set Tauri bundles. Never hand-draw a second version of
  the fox, and never commit a separate cropped or recoloured file.
- The native app icon is the fox on its own dark rounded tile, never the bare
  fox. Its fixed tile colour is set by `TILE` in the icon script independently
  of the workspace theme.
- The art is white on transparent with roughly 20% padding, so it only ever
  sits on a dark surface, and the `size` prop is the image box, not the fox.
  The fox fills about 70% of it; size the box accordingly.
- Replacing the logo is one step: overwrite that PNG (8-bit RGBA, square,
  non-interlaced) and run `node scripts/make-icon.mjs`.

## Shape and spacing

- `--r` (8px) for cards and panels. `--r-sm` (5px) for
  buttons, chips and inner groups. `999px` for status pills. Nothing else.
- Borders are 1px hairlines. Never 2px, never doubled.
- Launch is capped at 560px and the task composer at 680px. Working views use
  the available pane width; summary prose is capped at 75ch for readability.
- Vertical rhythm: 32px between sections, 16–18px inside a card.

## Type

- One sans (`--font`) and one mono (`--mono`). No third family, no webfont
  download — the app ships offline.
- Sizes: 30px launch hero, 20px screen title, 14px body, 12px navigation and
  metadata, 11px task captions and status-bar text.
  Nothing in between.
- `.label` (12px, medium weight, sentence case) heads secondary sections.
  Use spacing and type hierarchy instead of uppercase or decorative labels.
- Mono is for machine strings only: paths, versions, branch names, commands.
  Prose is never mono.
- Numbers use `font-variant-numeric: tabular-nums` so they do not jitter while
  streaming.

## Components

- **There is no component library and there will not be one.** Shared
  primitives live in `tokens.css` as plain classes: `.card`, `.btn`,
  `.btn.primary`, `.chip`, `.label`, `.mono`, `.note`, `.missing`, `.link`,
  `.grow`, `.dot`, `.diff`, `.code-view`, `.copy`. Everything else is scoped CSS
  in the file that uses it.
- **Do not extract a component until the third use.** Two copies of a row is
  cheaper than one wrong abstraction.
- One `.btn.primary` per screen, maximum. It is a light fill on dark, so it
  reads as the one obvious next step.
- **A destructive or consenting action is never the primary button.** "Trust
  and open" is a plain button in `--warn`; the safe choice is the easy one.
- Icons are inline SVG in the file that needs them. No icon package.
- The Project screen is a shell (`views/Project.vue`: sidebar, top bar) around
  one page at a time from `views/project/`: composer, live run, result, past
  task, AI helpers. All state lives in `views/project/state.ts` and the pages
  `inject` it. A page renders; it does not own logic.
- The sidebar holds only destinations that exist: New task, one row per live or
  just-finished run, AI helpers, recent tasks, switch project. No entry for a
  feature that is not wired up. Runs are concurrent, so a row per run is the
  only honest listing; each carries its own dot, its own label taken from what
  was asked, and opens its own stream or result.
- A status dot is green for done, `--err` for failed, `--warn` for checks that
  did not pass, blue while running, and neutral otherwise.
- A result shows what happened first; route, metrics, files and activity sit
  one tab away. Results sit directly on the page, with horizontally scrollable
  tabs when space is tight. The header's branch control combines change and
  sync status with a native Git popover. Forms, progress, errors and success
  stay in that popover without moving the workspace. Fetch runs on one click;
  actions that change files or publish work show their scope before submission.
  Results link directly to Commit or Merge with the task's branch selected.
  Git status refreshes when the popover opens, after tasks and on demand;
  incoming and outgoing counts explicitly reflect the last fetch.
- A reply continues the run it answers: same sidebar row, same page, the
  finished exchanges stacked above the current one with a hairline between them
  and no nested cards. The row keeps the name of what was first asked, and the
  heading shows what the user typed, never the recap sent to the CLI. A reply
  that had to open its own task — one whose run worked in a separate copy —
  gets its own row, because that is what happened. The reply box carries the
  composer's affordances: attach, add folder, paste, drop, Ctrl+Enter.
- A finished task in the sidebar takes a reply too, on the same terms. It opens
  a run showing the exchange it answers, and goes on in that same task. Its
  provider session is long cold, so the box says the reply reads the files again
  rather than picking up where it left off — it never implies a warm one.
- The composer separates writing and attachments from task settings. The AI,
  approach and working location stay visible while the settings are collapsed.
  “Run task” is the primary action, with a Ctrl+Enter hint on wider windows.
- Launch uses a centered wordmark, one project action, an open-project list and
  a simple recent project list. Projects stay open while another is on screen,
  so the open list says how many tasks each one has running and refuses to close
  one that is busy. Switching project closes nothing. The workspace has a 240px sidebar (208px in smaller desktop
  windows), a 44px pane header and a compact status bar. Task history nests
  under the current project and shows each task’s status and provider.
- The pane header identifies the current screen with a tab-like treatment.
  It is a heading, not a tab control. The footer reports the real project path,
  provider allowances and running state. Do not add decorative terminal panes,
  fake resource readings or destinations for features that do not exist.
- Claude and Codex usage counters show percentage **remaining**, labelled by
  window. Small neutral bars become amber at 20% or less. Each counter opens a
  native popover with every reported window, reset times and the last check.
  Missing readings, missing CLIs and signed-out accounts are explicit states.
  Refresh is manual or follows project opening, sign-in, installation and tasks;
  the UI never implies a live feed. A newer check supersedes any older response.
- The main pane and task history scroll independently. Activity and results
  sit on the page with simple dividers instead of nested cards.

## The dock

The dock is the half of the window that is not the conversation: real
terminals, the project's files, a file being edited and the dev server's own
page. It shares the workspace pane with the current screen, split by a
draggable divider, and `Ctrl+\`` shows and hides it.

- **Nothing in the dock is a picture of a tool.** Every terminal is a ConPTY
  running the user's own shell, the tree lists files that exist, and the
  preview frames an address a dev server actually answered on. The rule
  against decorative terminal panes stands — this is the real one.
- Tabs stay mounted while another is on top. A build keeps running, a file
  keeps its unsaved text, and a scrolled-back terminal keeps its place.
- A tab carries one mark at most: `•` in `--warn` for unsaved text, `□` for a
  shell that exited. Neither is an error colour, because neither is a failure.
- The dock's position (beside or below), its share of the pane, the shell, the
  mono face, type size, cursor and scrollback are the user's, saved across
  projects. Open tabs are saved per project. A blocked or full store loses the
  preference, never the session.
- **Type size inside the dock is a user setting, not the type scale.** It is
  applied in script, so the scale in this document keeps governing every
  `font-size` a component's stylesheet declares.
- **Terminal content colours belong to the programs printing them.** Orteca
  sets only the background, the resting text, the cursor and the selection from
  tokens, and leaves xterm's ANSI palette alone. Syntax highlighting is the one
  exception: it gets its own `--syntax-*` tokens rather than borrowing green
  and blue, which already mean outcome and in-flight.
- The working tree sits at the top of the history as its own row, opening the
  same patch view as a commit. It appears only when there is something
  uncommitted, and says it is not committed rather than borrowing a hash.
- History is a list or one commit, never both at once — the dock is too narrow
  to read a patch beside the thing it came from. Relative times are git's own
  wording (`3 hours ago`), never recomputed here, so they cannot disagree with
  what every other git tool says; the exact timestamp is the row's title.
- A patch is read, not decorated. Only added and removed lines are coloured,
  and they use the `--syntax-*` tokens — never outcome green, which would claim
  a commit succeeded at something.
- The preview frames nothing until it is given an address. It does not start a
  dev server, and it never claims a server is running — an empty frame says to
  start one in a terminal.

## Honesty (this one outranks looks)

- **Every number on screen carries its label and its quality.** A cost renders
  as `$0.0123 exact` / `estimated`, or as `—` with "cost unavailable". Never a
  bare figure.
- **Never render `0` for unknown.** Unknown is an em dash plus a word saying
  why.
- No savings percentage, no "62% less context", no "instead of 5 steps" until
  the project has a real recorded baseline to compare against. The mock shows
  those numbers; they are only allowed once they are measured.
- A missing CLI, a detached HEAD or an untrusted repo is a normal state with
  normal styling — not an error colour, not a red banner.

## Motion and access

- Transitions are 120ms ease, on colour and background only. Nothing moves,
  slides or bounces on mount.
- The only looping animation in the app is the run progress bar, and it is
  disabled under `prefers-reduced-motion`.
- Every interactive element is a real `<button>` / `<a>` / `<input>`. Focus
  uses the global `:focus-visible` ring; never remove an outline without
  replacing it.
- Every icon-only control has a `title` and, where the icon carries meaning,
  an `aria-label`. Decorative SVG gets `aria-hidden="true"`.
- Streams are height-capped and scroll inside their card. The page itself never
  grows a horizontal scrollbar.

## When the mocks and these rules disagree

The mocks show a future app (task history, routing insights, agent picker).
Build the screen that exists today in the mock's visual language — do not build
the mock's chrome for features that are not wired up yet.
