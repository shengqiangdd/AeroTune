import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const css = readFileSync("src/index.css", "utf8");
const ui = readFileSync("src/components/ui.tsx", "utf8");
const match = css.match(/@media \(forced-colors: active\)\s*\{([\s\S]*?)\n\}/);
assert.ok(match, "forced-colors override block must remain present");
const forcedColors = match[1];

assert.match(forcedColors, /\.glass\s*\{[\s\S]*color:\s*CanvasText;[\s\S]*background:\s*Canvas;[\s\S]*border:\s*1px solid CanvasText;/);
assert.match(forcedColors, /\.grad-text\s*\{[\s\S]*background:\s*none !important;[\s\S]*color:\s*CanvasText !important;[\s\S]*-webkit-text-fill-color:\s*CanvasText !important;/);
assert.match(forcedColors, /button\.grad-btn\s*\{[\s\S]*background:\s*ButtonFace !important;[\s\S]*color:\s*ButtonText !important;[\s\S]*border:\s*1px solid ButtonText;/);
assert.match(forcedColors, /:focus-visible\s*\{[\s\S]*outline:\s*2px solid Highlight !important;[\s\S]*outline-offset:\s*2px;[\s\S]*box-shadow:\s*none !important;/);
assert.doesNotMatch(forcedColors, /forced-color-adjust:\s*none/);
assert.match(ui, /focus-visible:outline-none/, "global forced-colors outline must override shared components' custom ring reset");

console.log("Forced-colors contract passed: system surfaces, visible brand text/buttons, and keyboard focus outlines");
