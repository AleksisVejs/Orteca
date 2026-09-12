import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import test from 'node:test';
import { parse } from '@vue/compiler-sfc';

// No preview is allowed for this review. These are source-level checks of
// docs/ui.md, not claims about rendered layout or visual accessibility.
const tokens = readFileSync(new URL('../src/styles/tokens.css', import.meta.url), 'utf8');
for (const folder of ['views', 'components']) {
  for (const file of readdirSync(new URL(`../src/${folder}/`, import.meta.url)).filter(f => f.endsWith('.vue'))) {
    const source = readFileSync(new URL(`../src/${folder}/${file}`, import.meta.url), 'utf8');
    const { descriptor } = parse(source);
    test(`${file} uses defined tokens and the documented font sizes`, () => {
      for (const { content } of descriptor.styles) {
        assert.doesNotMatch(content, /#[0-9a-f]{3,8}\b|\brgba?\(/i);
        for (const [, token] of content.matchAll(/var\((--[\w-]+)/g)) assert.ok(tokens.includes(`${token}:`), token);
        for (const [, size] of content.matchAll(/font-size:\s*(\d+)px/g)) assert.ok(['11', '12', '14', '20', '30'].includes(size), size);
      }
    });
    if (file === 'Launch.vue') test('launch hover does not claim a completed outcome', () => {
      assert.doesNotMatch(descriptor.styles.map(s => s.content).join('\n'), /var\(--accent\)/);
    });
  }
}
