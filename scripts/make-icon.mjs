// Generates src-tauri/icons/icon.ico — the Velo fox head, flat, sand on
// transparent. A 32x32 BGRA bitmap inside an ICO container; no dependencies.
//
// Placeholder resolution until Milestone 8 (packaging) needs the full set.
// Run: node scripts/make-icon.mjs

import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const SIZE = 32;
const SAND = [0xa4, 0xc8, 0xd9]; // BGRA order

// Fox head: a downward wedge with two ears. Coordinates are in a 32x32 grid.
const TRIANGLES = [
  [[3, 9], [29, 9], [16, 29]], // head
  [[3, 2], [12, 9], [3, 9]], // left ear
  [[29, 2], [29, 9], [20, 9]], // right ear
];

const sign = (px, py, a, b) =>
  (px - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (py - b[1]);

function inTriangle(px, py, [a, b, c]) {
  const d1 = sign(px, py, a, b);
  const d2 = sign(px, py, b, c);
  const d3 = sign(px, py, c, a);
  const neg = d1 < 0 || d2 < 0 || d3 < 0;
  const pos = d1 > 0 || d2 > 0 || d3 > 0;
  return !(neg && pos);
}

/** 4x supersampling so the diagonals are not jagged at 32px. */
function coverage(x, y) {
  let hits = 0;
  for (let sy = 0; sy < 4; sy++) {
    for (let sx = 0; sx < 4; sx++) {
      const px = x + (sx + 0.5) / 4;
      const py = y + (sy + 0.5) / 4;
      if (TRIANGLES.some((t) => inTriangle(px, py, t))) hits++;
    }
  }
  return hits / 16;
}

// XOR bitmap: bottom-up rows of BGRA.
const xor = Buffer.alloc(SIZE * SIZE * 4);
for (let y = 0; y < SIZE; y++) {
  for (let x = 0; x < SIZE; x++) {
    const alpha = Math.round(coverage(x, y) * 255);
    const offset = ((SIZE - 1 - y) * SIZE + x) * 4;
    xor[offset] = SAND[0];
    xor[offset + 1] = SAND[1];
    xor[offset + 2] = SAND[2];
    xor[offset + 3] = alpha;
  }
}

// AND mask: one bit per pixel, rows padded to 4 bytes. Zeroed — the alpha
// channel does the masking, but the field is still required by the format.
const andMask = Buffer.alloc(4 * SIZE);

const header = Buffer.alloc(40);
header.writeUInt32LE(40, 0); // biSize
header.writeInt32LE(SIZE, 4); // biWidth
header.writeInt32LE(SIZE * 2, 8); // biHeight: XOR + AND stacked
header.writeUInt16LE(1, 12); // biPlanes
header.writeUInt16LE(32, 14); // biBitCount
header.writeUInt32LE(xor.length + andMask.length, 20); // biSizeImage

const image = Buffer.concat([header, xor, andMask]);

const dir = Buffer.alloc(6);
dir.writeUInt16LE(0, 0); // reserved
dir.writeUInt16LE(1, 2); // type: icon
dir.writeUInt16LE(1, 4); // image count

const entry = Buffer.alloc(16);
entry[0] = SIZE; // width
entry[1] = SIZE; // height
entry.writeUInt16LE(1, 4); // planes
entry.writeUInt16LE(32, 6); // bit count
entry.writeUInt32LE(image.length, 8);
entry.writeUInt32LE(dir.length + entry.length, 12); // offset

const out = join(
  dirname(fileURLToPath(import.meta.url)),
  "..",
  "src-tauri",
  "icons",
);
mkdirSync(out, { recursive: true });
writeFileSync(join(out, "icon.ico"), Buffer.concat([dir, entry, image]));
console.log(`wrote ${join(out, "icon.ico")} (${dir.length + entry.length + image.length} bytes)`);
