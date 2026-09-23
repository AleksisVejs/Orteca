import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const source = readFileSync(new URL('../src/views/project/picks.ts', import.meta.url), 'utf8').replace(/^export /gm, '');
const { CLOSING, tidy, split, reference, forRouting } = vm.runInNewContext('(() => {' + ts.transpile(source, { target: ts.ScriptTarget.ES2022 }) + '; return { CLOSING, tidy, split, reference, forRouting }; })()');

const block = [
  'On http://127.0.0.1:8000/about, this element: <h2> "Darbojas ar RigInspect"',
  'Source file: C:/LiftMe/resources/js/components/RigInspectPanel.vue',
  'Selector: main#main > div > section:nth-of-type(1) > h2',
  'HTML: <h2 class="mt-3">Darbojas ar RigInspect</h2>',
  'Likely in the code (best first):',
  "  resources/js/locales/lv.js:845: title: 'Darbojas ar RigInspect',",
  'Change: Add made by Aleksis Vejs',
  CLOSING,
].join('\n');

test('a picked element shows as one line and comes back apart for editing', () => {
  const prompt = `${block}\n\nand keep it short`;
  assert.equal(tidy(prompt), 'Pointed at <h2> "Darbojas ar RigInspect": Add made by Aleksis Vejs\n\nand keep it short');
  const { picks, rest } = split(prompt);
  assert.equal(rest, 'and keep it short');
  assert.equal(picks.length, 1);
  assert.equal(picks[0].label, '<h2> "Darbojas ar RigInspect"');
  assert.equal(picks[0].block, block);
  assert.equal(tidy('plain words'), 'plain words');
});

test('an earlier task goes in whole, shows as one line, and routes on its title alone', () => {
  const ref = reference(12, 'Add pagination', ['add pagination to quotes', 'make it 20 a page'], 'Done.\nQuotes page at 20.', ['app/Quote.php']);
  assert.equal(ref.label, '#12 Add pagination');
  assert.match(ref.block, /Asked: add pagination to quotes\nThen: make it 20 a page\nFinal answer:\nDone\.\nQuotes page at 20\.\nFiles it changed: app\/Quote\.php/);
  const prompt = `${ref.block}\n\n${block}\n\nnow sort them by date`;
  assert.equal(tidy(prompt), 'Building on task #12: Add pagination\n\nPointed at <h2> "Darbojas ar RigInspect": Add made by Aleksis Vejs\n\nnow sort them by date');
  assert.equal(forRouting(prompt), `Attached earlier task #12: Add pagination\n\n${block}\n\nnow sort them by date`);
  const { picks, rest } = split(prompt);
  assert.equal(rest, 'now sort them by date');
  assert.equal(picks.map((p) => p.label).join(' | '), '<h2> "Darbojas ar RigInspect" | #12 Add pagination');
  assert.equal(picks[1].block, ref.block);
});

test('a task saved with the older heading still shows as one line', () => {
  const old = 'Earlier task #45 in this project, for context: Sentence builder issue\nAsked: why?\nFinal answer:\nBecause.\nEnd of the earlier task.\n\nsummarize it';
  assert.equal(tidy(old), 'Building on task #45: Sentence builder issue\n\nsummarize it');
  assert.equal(split(old).rest, 'summarize it');
});
