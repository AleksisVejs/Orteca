import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import ts from 'typescript';

const source = readFileSync(new URL('../src/markdown.ts', import.meta.url), 'utf8');
const js = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } }).outputText;
const { parseMarkdown } = await import(`data:text/javascript,${encodeURIComponent(js)}`);

test('an agent reply becomes headings, lists, paragraphs and code', () => {
  const blocks = parseMarkdown([
    'The card now has a **period** filter.',
    'It uses `User.php`.',
    '',
    '**How it works**',
    '- **The filter:** sits on top',
    '  and wraps here.',
    '- nested:',
    '  - one',
    '',
    '1. first',
    '2. second',
    '',
    '```',
    '<b>not html</b>',
    '```',
  ].join('\n'));
  assert.deepEqual(blocks.map(b => b.kind), ['p', 'p', 'list', 'code']);
  assert.equal(blocks[0].lines.length, 2);
  assert.deepEqual(blocks[0].lines[0][1], { kind: 'bold', text: 'period' });
  assert.deepEqual(blocks[0].lines[1][1], { kind: 'code', text: 'User.php' });
  assert.deepEqual(blocks[2].items.map(i => [i.depth, i.marker]), [[0, '•'], [0, '•'], [1, '•'], [0, '1.'], [0, '2.']]);
  assert.equal(blocks[2].items[0].spans.map(s => s.text).join(''), 'The filter: sits on top and wraps here.');
  assert.equal(blocks[3].text, '<b>not html</b>');
});

test('a heading and a link keep their words only', () => {
  const [h, p] = parseMarkdown('## Done\nSee [the file](src/a.ts).');
  assert.equal(h.kind, 'h');
  assert.equal(p.lines[0].map(s => s.text).join(''), 'See the file.');
});
