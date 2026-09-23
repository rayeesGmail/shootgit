// Generates the 1024x1024 placeholder app icon that `pnpm tauri icon` turns
// into src-tauri/icons/*. The product name and brand are still open questions
// (SPEC §12), so this is deliberately a plain mark, not a logo.
//
// It writes a PNG by hand (no image dependency, nothing installed globally):
//   node scripts/icons/make-placeholder-icon.mjs src-tauri/icons/source.png
//   pnpm tauri icon src-tauri/icons/source.png
//
// Redrawing it must be deterministic: same input, same bytes.

import { deflateSync } from 'node:zlib';
import { writeFileSync } from 'node:fs';
import { Buffer } from 'node:buffer';
import process from 'node:process';

const SIZE = 1024;
const SS = 3; // supersampling factor, for antialiasing
const RADIUS = 184; // rounded-square corner radius
const BG = [0x1f, 0x24, 0x30, 0xff];
const LINE = [0xe8, 0xe8, 0xee, 0xff];
const ACCENT = [0x7a, 0xa2, 0xf7, 0xff];

/** Distance from a point to a line segment. */
function distanceToSegment(px, py, ax, ay, bx, by) {
  const dx = bx - ax;
  const dy = by - ay;
  const lengthSquared = dx * dx + dy * dy;
  const t =
    lengthSquared === 0
      ? 0
      : Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / lengthSquared));
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}

function insideRoundedSquare(x, y) {
  const cx = Math.min(Math.max(x, RADIUS), SIZE - RADIUS);
  const cy = Math.min(Math.max(y, RADIUS), SIZE - RADIUS);
  return Math.hypot(x - cx, y - cy) <= RADIUS;
}

// A minimal commit graph: a trunk with three commits and one branch.
const TRUNK_X = 396;
const BRANCH_X = 652;
const STROKE = 30;
const DOT = 52;
const segments = [
  { a: [TRUNK_X, 268], b: [TRUNK_X, 756], color: LINE },
  { a: [TRUNK_X, 512], b: [BRANCH_X, 380], color: ACCENT },
  { a: [BRANCH_X, 380], b: [BRANCH_X, 268], color: ACCENT },
];
const dots = [
  { at: [TRUNK_X, 268], color: LINE },
  { at: [TRUNK_X, 512], color: LINE },
  { at: [TRUNK_X, 756], color: LINE },
  { at: [BRANCH_X, 268], color: ACCENT },
];

function sample(x, y) {
  if (!insideRoundedSquare(x, y)) return null;
  for (const dot of dots) {
    if (Math.hypot(x - dot.at[0], y - dot.at[1]) <= DOT / 2) return dot.color;
  }
  for (const segment of segments) {
    const d = distanceToSegment(x, y, ...segment.a, ...segment.b);
    if (d <= STROKE / 2) return segment.color;
  }
  return BG;
}

const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
for (let y = 0; y < SIZE; y += 1) {
  const rowStart = y * (SIZE * 4 + 1);
  raw[rowStart] = 0; // filter type: none
  for (let x = 0; x < SIZE; x += 1) {
    let r = 0;
    let g = 0;
    let b = 0;
    let a = 0;
    for (let sy = 0; sy < SS; sy += 1) {
      for (let sx = 0; sx < SS; sx += 1) {
        const pixel = sample(x + (sx + 0.5) / SS, y + (sy + 0.5) / SS);
        if (pixel !== null) {
          r += pixel[0];
          g += pixel[1];
          b += pixel[2];
          a += pixel[3];
        }
      }
    }
    const samples = SS * SS;
    const offset = rowStart + 1 + x * 4;
    // Premultiplied average, so partially covered edge pixels fade out.
    raw[offset] = a === 0 ? 0 : Math.round(r / (a / 255));
    raw[offset + 1] = a === 0 ? 0 : Math.round(g / (a / 255));
    raw[offset + 2] = a === 0 ? 0 : Math.round(b / (a / 255));
    raw[offset + 3] = Math.round(a / samples);
  }
}

function crc32(buffer) {
  let crc = 0xffffffff;
  for (const byte of buffer) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const length = Buffer.alloc(4);
  length.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body));
  return Buffer.concat([length, body, crc]);
}

const header = Buffer.alloc(13);
header.writeUInt32BE(SIZE, 0);
header.writeUInt32BE(SIZE, 4);
header[8] = 8; // bit depth
header[9] = 6; // colour type: RGBA
const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  chunk('IHDR', header),
  chunk('IDAT', deflateSync(raw, { level: 9 })),
  chunk('IEND', Buffer.alloc(0)),
]);

const out = process.argv[2];
if (out === undefined) {
  throw new Error('usage: node make-placeholder-icon.mjs <output.png>');
}
writeFileSync(out, png);
