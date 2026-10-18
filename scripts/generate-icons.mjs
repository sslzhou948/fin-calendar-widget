// scripts/generate-icons.mjs
// 纯 Node 生成应用图标（零第三方依赖）：在 CI 上现场生成 Tauri 所需图标。
// 原因：图标是二进制文件，GitHub App 无法推送到仓库；改为构建时生成。
// 用法：node scripts/generate-icons.mjs   → 输出 src-tauri/icons/icon.png（512x512）
// CI 里接着跑：npx tauri icon src-tauri/icons/icon.png --output src-tauri/icons
import { writeFileSync, mkdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { deflateSync } from 'node:zlib';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const OUT_DIR = join(ROOT, 'src-tauri', 'icons');
mkdirSync(OUT_DIR, { recursive: true });

const S = 512;
const buf = Buffer.alloc(S * S * 4); // RGBA，初始全透明

function blendPx(x, y, r, g, b, a) {
  x = Math.round(x); y = Math.round(y);
  if (x < 0 || y < 0 || x >= S || y >= S || a <= 0) return;
  const i = (y * S + x) * 4;
  const sa = a / 255, da = buf[i + 3] / 255;
  const oa = sa + da * (1 - sa);
  if (oa <= 0) return;
  buf[i]     = Math.round((r * sa + buf[i]     * da * (1 - sa)) / oa);
  buf[i + 1] = Math.round((g * sa + buf[i + 1] * da * (1 - sa)) / oa);
  buf[i + 2] = Math.round((b * sa + buf[i + 2] * da * (1 - sa)) / oa);
  buf[i + 3] = Math.round(oa * 255);
}

// 圆角矩形 SDF（含抗锯齿）
function fillRoundedRect(cx, cy, w, h, r, color) {
  const [cr, cg, cb, ca] = color;
  const x0 = Math.floor(cx - w / 2 - 1), x1 = Math.ceil(cx + w / 2 + 1);
  const y0 = Math.floor(cy - h / 2 - 1), y1 = Math.ceil(cy + h / 2 + 1);
  for (let y = y0; y <= y1; y++) {
    for (let x = x0; x <= x1; x++) {
      const dx = Math.abs(x - cx) - w / 2 + r;
      const dy = Math.abs(y - cy) - h / 2 + r;
      const d = Math.hypot(Math.max(dx, 0), Math.max(dy, 0)) + Math.min(Math.max(dx, dy), 0) - r;
      if (d < 1) {
        const alpha = ca * (d <= 0 ? 1 : 1 - d);
        blendPx(x, y, cr, cg, cb, alpha);
      }
    }
  }
}

// 实心圆（含抗锯齿）
function fillCircle(cx, cy, rad, color) {
  const [cr, cg, cb, ca] = color;
  const x0 = Math.floor(cx - rad - 1), x1 = Math.ceil(cx + rad + 1);
  const y0 = Math.floor(cy - rad - 1), y1 = Math.ceil(cy + rad + 1);
  for (let y = y0; y <= y1; y++) {
    for (let x = x0; x <= x1; x++) {
      const d = Math.hypot(x - cx, y - cy) - rad;
      if (d < 1) {
        const alpha = ca * (d <= 0 ? 1 : 1 - d);
        blendPx(x, y, cr, cg, cb, alpha);
      }
    }
  }
}

// ---- 设计：深色圆角面板 + 日历数据点阵（呼应财经日历），一颗绿色高亮 ----
const SLATE = [30, 41, 59, 255];      // #1e293b
const SLATE_LT = [51, 65, 85, 255];   // #334155（描边）
const DOT = [203, 213, 225, 255];     // #cbd5e1
const GREEN = [34, 197, 94, 255];      // #22c55e（超预期绿，品牌色）

fillRoundedRect(256, 256, 368, 320, 72, SLATE);
// 顶部装饰条（模拟挂件标题栏）
fillRoundedRect(256, 150, 368, 64, 30, [15, 23, 42, 255]);
// 数据点阵 4x3
const cols = [145, 219, 293, 367], rows = [248, 322, 396];
for (let r = 0; r < 3; r++) {
  for (let c = 0; c < 4; c++) {
    const accent = (r === 1 && c === 2);
    fillCircle(cols[c], rows[r], accent ? 30 : 24, accent ? GREEN : DOT);
  }
}

// ---- 最小编码器：RGBA → PNG ----
function crc32(b) {
  let t = crc32.table;
  if (!t) {
    t = crc32.table = new Int32Array(256);
    for (let n = 0; n < 256; n++) {
      let c = n;
      for (let k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
      t[n] = c;
    }
  }
  let c = 0xffffffff;
  for (let i = 0; i < b.length; i++) c = t[(c ^ b[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4); crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}
const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0); ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; // 8-bit, RGBA
const raw = Buffer.alloc(S * (1 + S * 4));
for (let y = 0; y < S; y++) {
  raw[y * (1 + S * 4)] = 0; // filter type 0
  buf.copy(raw, y * (1 + S * 4) + 1, y * S * 4, (y + 1) * S * 4);
}
const png = Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
const outPath = join(OUT_DIR, 'icon.png');
writeFileSync(outPath, png);
console.log('wrote', outPath, png.length, 'bytes');
