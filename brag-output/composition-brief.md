# Hyperframes Composition Brief: Orteca

## Objective
Instagram-reel meme ad for Orteca.

## Output
- Composition: `brag-output/composition/` · Render: `brag-output/brag.mp4`
- Vertical 1080x1920 · 23.5s

## Source Material
- Files read: `src/styles/tokens.css`, `docs/ui.md`, `docs/architecture.md`, `src/views/Launch.vue`, `src/views/project/TaskComposer.vue`, `TaskResult.vue`, `TaskRun.vue`, `CLAUDE.md`
- Verbatim copy: "The efficient way to run coding agents." · "One prompt in. Finished work out." · "What do you want to build?" · "Example: Add dark mode and make sure it works." · "Do it" · cost labels `exact` / `estimated`
- Logo: `assets/img/velo.png` (white fox, on the `--surface` tile)

## Creative Direction
- Tone: chaotic · direction: Instagram meme reel. See brag-plan.md.
- Avoid: generic SaaS copy, abstract filler, fake green numbers used as decoration.

## Visual Identity
Tokens from `tokens.css` (see plan). Fonts shipped locally: Segoe UI Bold, Cascadia Code.

## Storyboard
brag-plan.md scenes 1–8.

## Audio
- Music: vol-10 at 0.38, fade out last 1.2s. Cue preset JSON in brag skill assets. One beat-lock: outro at 20.19s.
- SFX: chosen per motion (see plan), copied into `assets/sfx/`.
- Audio-reactive: skipped — the extraction script needs a Python/ffmpeg setup this machine does not have on PATH; plain beat-sync instead.
