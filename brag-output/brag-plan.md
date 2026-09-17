# Brag Plan: Orteca

## What is this app?
A Windows desktop app that takes one plain-English prompt, routes it through `claude`/`codex` agent stages (Plan → Implement → Review → Verify, with one Fix round), and reports cost with an honest `exact` / `estimated` / `unavailable` label.

## The angle
Instagram-reel meme ad. Dev-meme captions ("POV:", "nobody: / me:") over fake-but-faithful Orteca UI. The joke is real pain every agent user has: four terminals of babysitting, Ctrl+C that leaves `npm` alive, and tools bragging "SAVED 300%". Orteca's answers are real: stages, a Job Object that kills the whole tree, and cost labels that refuse to lie.

## Hook (first 3 seconds)
Top caption "POV: you're babysitting 4 AI agents in 4 terminals" while terminal windows slam in, tilted, stacking into a mess.

## Key moments
- The composer: "What do you want to build?" types "Add dark mode and make sure it works." → cursor clicks **Do it**.
- Stage chips land one by one: Plan · Implement · Review · Verify. Verify fails ✗ → Fix → Verify ✓ (green).
- Cost card: "SAVED 300% 🚀" gets struck out; replaced by `$0.41 · exact`.
- Stop: node → bash → npm rows all die. "cancelled", not "failed".

## Outro / punchline
Fox logo + "Orteca" + "One prompt in. Finished work out."

## User flow worth showing
Type prompt → Do it → stages run (fail, fix, pass) → result with labelled cost.

## Tone
- Preset: chaotic
- Creative direction: Instagram reel meme ad, trendy, captions-as-jokes
- Interpretation: fast slams, tilted meme captions, stacked SFX, but every caption holds long enough to read.

## Format: vertical — 1080x1920
## Duration: 23.5s

## Visual identity (from src/styles/tokens.css)
- Background: #0a0e11 · surface #10161a · surface-2 #1c242d · border #222c34
- Accent (outcome): #3ecf8e · Info (in flight): #5b83f7 · Err: #e0715f · Warn: #d4a24c
- Text: #e8eaed / #b1bdcc / #8e9baa
- Display/body font: Segoe UI (bold) · Mono: Cascadia Code
- Strongest visual: the Velo fox mark on its dark tile; stage chips; cost-quality label
- Meme captions: white text, black stroke, the one place we break the app's quiet rules.

## Share copy (draft)
I got tired of babysitting four AI terminals, so I built the babysitter. Orteca: one prompt in, finished work out.

## Audio direction
- Role: dense rhythmic layer
- Music: happy-beats-business-moves-vol-10 (punchy, 109.96 BPM), 0.38, fade out at end
- Music cue guidance: preset `assets/music/cues/happy-beats-business-moves-vol-10-by-ende-dot-app.music-cues.json`. Strong cue 20.19s → outro logo slam. Beat grid 7.79 / 8.22 / 8.73 / 9.29 region for typing; chips on every other beat from 10.38 (10.38, 11.47, 12.56 is too slow; use 10.38, 10.93, 11.47, 12.02 with the full set held after).
- Audio-reactive: skipped (see brief).
- SFX posture: dense, motion-matched: punches on terminal slams, keypresses on typing, click on Do it, card-place on chips, error buzz on ✗, chips-collide on ✓, glitch on strikeout, bell on logo.
- Restraint rule: no SFX under readable caption holds beyond the entrance hit.

## Storyboard

### Scene 1 — POV hook — 0.0–3.3s
Caption "POV: you're babysitting 4 AI agents in 4 terminals". 4 terminal windows slam in tilted (claude / codex / claude / npm test) with scrolling JSONL-looking lines.
Sequential: yes, 4 windows on beats. Audio: punch per window. → hard cut

### Scene 2 — Ctrl+C meme — 3.3–6.3s
"you: Ctrl+C" then "the npm grandchild:" + ghost row `node → bash → npm  ● still running`.
Audio: click, then error buzz. → glitch cut

### Scene 3 — Reveal — 6.3–8.0s
Fox tile slams in, "ORTECA", "The efficient way to run coding agents." Audio: soft impact. → zoom cut

### Scene 4 — Composer — 8.0–11.8s
"What do you want to build?" card; prompt types; cursor clicks Do it. Caption top: "me now:". Audio: keypresses, mouse click.

### Scene 5 — Stages — 11.8–15.6s
Chips Plan · Implement · Review · Verify arrive; blue progress bar; Verify ✗ red → "Fix" chip → Verify ✓ green. Caption "it checks its own homework". Audio: card-place per chip, error, chips-collide.

### Scene 6 — Honest cost — 15.6–18.4s
"SAVED 300% 🚀" struck → "$0.41 · exact", "codex tokens · estimated". Caption "no fake savings. ever.". Audio: glitch, chip-lay.

### Scene 7 — Stop — 18.4–20.2s
Stop button click → node/bash/npm rows struck → status "cancelled". Caption "stop means STOP". Audio: punch.

### Scene 8 — Outro — 20.2–23.5s (beat-locked 20.19)
Fox + "Orteca" + "One prompt in. Finished work out." + "for claude + codex · Windows". Audio: bell.

**Music mood:** chaotic/upbeat. **Audio summary:** punchy bed with dense motion-matched hits, landing on a bell for the logo.
