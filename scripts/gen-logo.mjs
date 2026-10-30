// 生成 AeroTune 应用图标（纯 Node，无第三方依赖）
// 设计：深色底 + 青→紫渐变 + 白色仪表弧线，呼应"系统调优"主题
import zlib from "node:zlib";
import fs from "node:fs";
import path from "node:path";

const SIZE = 1024;

const C1 = [34, 211, 238];   // cyan  #22d3ee
const C2 = [139, 92, 246];   // violet #8b5cf6
const BG1 = [13, 17, 28];    // #0d111c
const BG2 = [30, 24, 58];    // #1e183a

function mix(a, b, t) {
  return [
    Math.round(a[0] + (b[0] - a[0]) * t),
    Math.round(a[1] + (b[1] - a[1]) * t),
    Math.round(a[2] + (b[2] - a[2]) * t),
  ];
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(zlib.crc32(body) >>> 0);
  return Buffer.concat([len, body, crc]);
}

function render(c1, c2) {
  const cx = SIZE / 2, cy = SIZE / 2;
  const R_IN = 300, R_OUT = 372;
  const raw = Buffer.alloc(SIZE * (SIZE * 4 + 1));
  for (let y = 0; y < SIZE; y++) {
    const rowStart = y * (SIZE * 4 + 1);
    raw[rowStart] = 0; // filter: none
    for (let x = 0; x < SIZE; x++) {
      const t = (x + y) / (2 * SIZE);
      let [r, g, b] = mix(BG1, BG2, t);

      const dx = x - cx, dy = y - cy;
      const dist = Math.hypot(dx, dy);
      let ang = Math.atan2(dy, dx) * 180 / Math.PI;
      if (dist >= R_IN && dist <= R_OUT) {
        const inMain = ang >= -210 && ang <= 30;
        if (inMain) [r, g, b] = mix(mix(c1, c2, ang / 240 + 0.5), [255, 255, 255], 0.55);
        else [r, g, b] = mix([r, g, b], [255, 255, 255], 0.16);
      }
      if (dist >= R_IN - 26 && dist <= R_OUT + 26) {
        for (let i = 0; i < 5; i++) {
          const a = -210 + i * 60;
          if (Math.abs(ang - a) < 1.2) [r, g, b] = mix([r, g, b], [255, 255, 255], 0.5);
        }
      }
      if (dist < 210) {
        const needle = Math.abs(ang - 30) < 2.2 && dist > 60;
        if (needle) [r, g, b] = [240, 250, 255];
        if (dist < 46) [r, g, b] = [240, 250, 255];
      }
      const off = rowStart + 1 + x * 4;
      raw[off] = r; raw[off + 1] = g; raw[off + 2] = b; raw[off + 3] = 255;
    }
  }

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(SIZE, 0);
  ihdr.writeUInt32BE(SIZE, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", zlib.deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

function writePng(file, buf) {
  fs.writeFileSync(file, buf);
  console.log("written", file, buf.length, "bytes");
}

// 主图标（青→紫）
writePng(path.join("scripts", "logo.png"), render(C1, C2));
// 托盘健康状态变体（绿 / 琥珀 / 红）
writePng(path.join("src-tauri", "icons", "tray-ok.png"), render([52, 211, 153], [34, 211, 238]));
writePng(path.join("src-tauri", "icons", "tray-warn.png"), render([251, 191, 36], [249, 115, 22]));
writePng(path.join("src-tauri", "icons", "tray-bad.png"), render([248, 113, 113], [239, 68, 68]));

