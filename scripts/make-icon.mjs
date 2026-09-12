// Generates the whole src-tauri/icons set from src/assets/velo.png — the one
// source of truth for the mark. The PNG is white on transparent, so it is
// composited onto the dark rounded tile from the brand icon; a bare white fox
// would vanish on a light taskbar.
//
// Node stdlib only: zlib inflates the PNG and deflates the 256px entry back.
// Run: node scripts/make-icon.mjs

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { crc32, deflateSync, inflateSync } from "node:zlib";

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = join(here, "..", "src", "assets", "velo.png");
const ICO_SIZES = [16, 32, 48, 256];
// Tauri's conventional PNG set. Windows ships the .ico; these are what the
// other bundlers and the dev window reach for.
const PNG_FILES = {
  "32x32.png": 32,
  "128x128.png": 128,
  "128x128@2x.png": 256,
  "icon.png": 512,
};

const TILE = [0x0d, 0x10, 0x12]; // RGB, matches --surface in tokens.css
const RIM = 0.16; // white rim opacity, as in the brand icon
const RADIUS = 0.225; // corner radius as a fraction of the tile
const INSET = 0.72; // how much of the tile the fox fills

/** Decode a non-interlaced 8-bit RGBA PNG. That is all this mark needs. */
function decodePng(file) {
  const buf = readFileSync(file);
  let at = 8;
  let head;
  const idat = [];
  while (at < buf.length) {
    const len = buf.readUInt32BE(at);
    const type = buf.toString("ascii", at + 4, at + 8);
    const data = buf.subarray(at + 8, at + 8 + len);
    if (type === "IHDR") {
      head = {
        w: data.readUInt32BE(0),
        h: data.readUInt32BE(4),
        depth: data[8],
        color: data[9],
        interlace: data[12],
      };
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    at += 12 + len;
  }
  if (!head) throw new Error(`${file}: no IHDR`);
  if (head.depth !== 8 || head.color !== 6 || head.interlace !== 0) {
    throw new Error(
      `${file}: expected 8-bit RGBA, non-interlaced (got depth ${head.depth}, ` +
        `colour type ${head.color}, interlace ${head.interlace})`,
    );
  }

  const { w, h } = head;
  const stride = w * 4;
  const raw = inflateSync(Buffer.concat(idat));
  const px = Buffer.alloc(h * stride);
  let p = 0;
  for (let y = 0; y < h; y++) {
    const filter = raw[p++];
    const line = raw.subarray(p, p + stride);
    p += stride;
    const cur = px.subarray(y * stride, (y + 1) * stride);
    const prev = y ? px.subarray((y - 1) * stride, y * stride) : null;
    for (let i = 0; i < stride; i++) {
      const a = i >= 4 ? cur[i - 4] : 0;
      const b = prev ? prev[i] : 0;
      const c = i >= 4 && prev ? prev[i - 4] : 0;
      let v = line[i];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const guess = a + b - c;
        const da = Math.abs(guess - a);
        const db = Math.abs(guess - b);
        const dc = Math.abs(guess - c);
        v += da <= db && da <= dc ? a : db <= dc ? b : c;
      }
      cur[i] = v & 0xff;
    }
  }
  return { w, h, px };
}

/** Crop to the mark's alpha bounding box — the source PNG has loose padding. */
function trim(src) {
  let x0 = src.w;
  let y0 = src.h;
  let x1 = -1;
  let y1 = -1;
  for (let y = 0; y < src.h; y++) {
    for (let x = 0; x < src.w; x++) {
      if (src.px[(y * src.w + x) * 4 + 3] <= 8) continue;
      if (x < x0) x0 = x;
      if (x > x1) x1 = x;
      if (y < y0) y0 = y;
      if (y > y1) y1 = y;
    }
  }
  if (x1 < 0) throw new Error("source image is fully transparent");
  const w = x1 - x0 + 1;
  const h = y1 - y0 + 1;
  const px = Buffer.alloc(w * h * 4);
  for (let y = 0; y < h; y++) {
    const from = ((y + y0) * src.w + x0) * 4;
    src.px.copy(px, y * w * 4, from, from + w * 4);
  }
  return { w, h, px };
}

/** Box downscale on premultiplied alpha, so edges do not pick up a dark halo. */
function resize(src, dw, dh) {
  const out = Buffer.alloc(dw * dh * 4);
  const sx = src.w / dw;
  const sy = src.h / dh;
  for (let y = 0; y < dh; y++) {
    const y0 = Math.floor(y * sy);
    const y1 = Math.max(y0 + 1, Math.floor((y + 1) * sy));
    for (let x = 0; x < dw; x++) {
      const x0 = Math.floor(x * sx);
      const x1 = Math.max(x0 + 1, Math.floor((x + 1) * sx));
      let r = 0;
      let g = 0;
      let b = 0;
      let a = 0;
      let n = 0;
      for (let yy = y0; yy < y1; yy++) {
        for (let xx = x0; xx < x1; xx++) {
          const i = (yy * src.w + xx) * 4;
          const alpha = src.px[i + 3] / 255;
          r += src.px[i] * alpha;
          g += src.px[i + 1] * alpha;
          b += src.px[i + 2] * alpha;
          a += alpha;
          n++;
        }
      }
      const o = (y * dw + x) * 4;
      if (a > 0) {
        out[o] = Math.round(r / a);
        out[o + 1] = Math.round(g / a);
        out[o + 2] = Math.round(b / a);
      }
      out[o + 3] = Math.round((a / n) * 255);
    }
  }
  return out;
}

/** Coverage of a rounded square at (x, y), 4x supersampled for smooth corners. */
function tileCoverage(x, y, size, inset) {
  const r = size * RADIUS;
  const lo = inset;
  const hi = size - inset;
  let hits = 0;
  for (let sy = 0; sy < 4; sy++) {
    for (let sx = 0; sx < 4; sx++) {
      const px = x + (sx + 0.5) / 4;
      const py = y + (sy + 0.5) / 4;
      if (px < lo || px > hi || py < lo || py > hi) continue;
      const cx = Math.min(Math.max(px, lo + r), hi - r);
      const cy = Math.min(Math.max(py, lo + r), hi - r);
      const dx = px - cx;
      const dy = py - cy;
      if (dx * dx + dy * dy <= r * r) hits++;
    }
  }
  return hits / 16;
}

/** Dark tile, thin rim, fox centred on top. Returns straight-alpha RGBA. */
function compose(fox, size) {
  const box = size * INSET;
  const scale = box / Math.max(fox.w, fox.h);
  const fw = Math.max(1, Math.round(fox.w * scale));
  const fh = Math.max(1, Math.round(fox.h * scale));
  const scaled = resize(fox, fw, fh);
  const ox = Math.round((size - fw) / 2);
  const oy = Math.round((size - fh) / 2);
  const rim = Math.max(1, Math.round(size / 32));

  const out = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const o = (y * size + x) * 4;
      const outer = tileCoverage(x, y, size, 0);
      if (outer === 0) continue;
      // The rim is the sliver between the tile edge and the same shape inset.
      const edge = Math.max(0, outer - tileCoverage(x, y, size, rim)) * RIM;
      out[o] = Math.round(TILE[0] * (1 - edge) + 255 * edge);
      out[o + 1] = Math.round(TILE[1] * (1 - edge) + 255 * edge);
      out[o + 2] = Math.round(TILE[2] * (1 - edge) + 255 * edge);
      out[o + 3] = Math.round(outer * 255);

      const fx = x - ox;
      const fy = y - oy;
      if (fx < 0 || fy < 0 || fx >= fw || fy >= fh) continue;
      const f = (fy * fw + fx) * 4;
      const a = scaled[f + 3] / 255;
      if (a === 0) continue;
      for (let c = 0; c < 3; c++) {
        out[o + c] = Math.round(out[o + c] * (1 - a) + scaled[f + c] * a);
      }
      out[o + 3] = Math.max(out[o + 3], scaled[f + 3]);
    }
  }
  return out;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const sum = Buffer.alloc(4);
  sum.writeUInt32BE(crc32(body));
  return Buffer.concat([len, body, sum]);
}

/** Entries above 48px are stored as PNG; Windows expects that for 256. */
function encodePng(rgba, size) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  const stride = size * 4;
  const raw = Buffer.alloc(size * (stride + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (stride + 1)] = 0; // filter: none
    rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

/** Bottom-up BGRA bitmap plus the AND mask the format still demands. */
function encodeBmp(rgba, size) {
  const head = Buffer.alloc(40);
  head.writeUInt32LE(40, 0);
  head.writeInt32LE(size, 4);
  head.writeInt32LE(size * 2, 8); // XOR and AND stacked
  head.writeUInt16LE(1, 12);
  head.writeUInt16LE(32, 14);

  const xor = Buffer.alloc(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const i = (y * size + x) * 4;
      const o = ((size - 1 - y) * size + x) * 4;
      xor[o] = rgba[i + 2];
      xor[o + 1] = rgba[i + 1];
      xor[o + 2] = rgba[i];
      xor[o + 3] = rgba[i + 3];
    }
  }
  const mask = Buffer.alloc((((size + 31) >> 5) * 4) * size);
  head.writeUInt32LE(xor.length + mask.length, 20);
  return Buffer.concat([head, xor, mask]);
}

const fox = trim(decodePng(SOURCE));
const out = join(here, "..", "src-tauri", "icons");
mkdirSync(out, { recursive: true });

const images = ICO_SIZES.map((size) => {
  const rgba = compose(fox, size);
  return { size, body: size > 48 ? encodePng(rgba, size) : encodeBmp(rgba, size) };
});

const dir = Buffer.alloc(6);
dir.writeUInt16LE(1, 2); // type: icon
dir.writeUInt16LE(images.length, 4);

let offset = dir.length + images.length * 16;
const entries = images.map(({ size, body }) => {
  const entry = Buffer.alloc(16);
  entry[0] = size & 0xff; // 0 means 256
  entry[1] = size & 0xff;
  entry.writeUInt16LE(1, 4);
  entry.writeUInt16LE(32, 6);
  entry.writeUInt32LE(body.length, 8);
  entry.writeUInt32LE(offset, 12);
  offset += body.length;
  return entry;
});

const ico = Buffer.concat([dir, ...entries, ...images.map((i) => i.body)]);
writeFileSync(join(out, "icon.ico"), ico);
console.log(`icon.ico  ${ico.length} bytes (${ICO_SIZES.join("/")}px)`);

for (const [name, size] of Object.entries(PNG_FILES)) {
  const png = encodePng(compose(fox, size), size);
  writeFileSync(join(out, name), png);
  console.log(`${name.padEnd(14)} ${png.length} bytes (${size}px)`);
}
