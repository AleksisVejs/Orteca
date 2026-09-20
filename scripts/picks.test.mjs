import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import test from 'node:test';
import ts from 'typescript';

const source = readFileSync(new URL('../src/views/project/picks.ts', import.meta.url), 'utf8').replace(/^export /gm, '');
const { CLOSING, tidy, split } = vm.runInNewContext('(() => {' + ts.transpile(source, { target: ts.ScriptTarget.ES2022 }) + '; return { CLOSING, tidy, split }; })()');

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
