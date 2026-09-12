# UI rules

The look is set by the Orteca mock set and the Velo icon: a cool near-black
desktop app, near-white text, one green for outcomes, one blue for work in
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
- Backgrounds are flat. No gradients, no glass, no blur. The single exception
  is the logo tile on the launch screen, which gets a rim and a drop shadow
  because it is the product mark.

## The mark

- `src/assets/velo.png` is the only copy of the fox. `VeloMark.vue` renders it
  and `scripts/make-icon.mjs` builds every file in `src-tauri/icons/` from it —
  `icon.ico` plus the PNG set Tauri bundles. Never hand-draw a second version of
  the fox, and never commit a separate cropped or recoloured file.
- The app icon is the fox on the dark rounded tile, never the bare fox. The tile
  colour tracks `--surface`; if that token changes, change `TILE` in the script
  and regenerate.
- The art is white on transparent with roughly 20% padding, so it only ever
  sits on a dark surface, and the `size` prop is the image box, not the fox.
  The fox fills about 70% of it; size the box accordingly.
- Replacing the logo is one step: overwrite that PNG (8-bit RGBA, square,
  non-interlaced) and run `node scripts/make-icon.mjs`.

## Shape and spacing

- `--r` (10px) for cards, panels and the logo tile. `--r-sm` (6px) for
  buttons, chips and inner groups. `999px` for status pills. Nothing else.
- Borders are 1px hairlines. Never 2px, never doubled.
- Content columns are capped: 560px on Launch, 720px on Project. Full-bleed
  layouts are not part of this app.
- Vertical rhythm: 32px between sections, 16–18px inside a card.

## Type

- One sans (`--font`) and one mono (`--mono`). No third family, no webfont
  download — the app ships offline.
- Sizes: 30px hero, 20px screen title, 14px body, 12px metadata, 11px label.
  Nothing in between.
- `.label` (11px, uppercase, tracked, faint) heads every section. Section
  headings are not styled as body text and vice versa.
- Mono is for machine strings only: paths, versions, branch names, commands.
  Prose is never mono.
- Numbers use `font-variant-numeric: tabular-nums` so they do not jitter while
  streaming.

## Components

- **There is no component library and there will not be one.** Shared
  primitives live in `tokens.css` as plain classes: `.card`, `.btn`,
  `.btn.primary`, `.chip`, `.label`, `.mono`. Everything else is scoped CSS in
  the view that uses it.
- **Do not extract a component until the third use.** Two copies of a row is
  cheaper than one wrong abstraction.
- One `.btn.primary` per screen, maximum. It is a light fill on dark, so it
  reads as the one obvious next step.
- **A destructive or consenting action is never the primary button.** "Trust
  and open" is a plain button in `--warn`; the safe choice is the easy one.
- Icons are inline SVG in the file that needs them. No icon package.
- No sidebar, no tab bar, no breadcrumb until there are genuinely more than
  three destinations. The mock set shows a sidebar for a five-screen app; this
  app has two screens and a modal, and fake navigation is worse than none.

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
