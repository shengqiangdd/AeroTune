import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const config = JSON.parse(readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"));
const html = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const main = readFileSync(new URL("../src/main.tsx", import.meta.url), "utf8");
const viteConfig = readFileSync(new URL("../vite.config.ts", import.meta.url), "utf8");

function sourceFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(?:[cm]?[jt]sx?|css)$/.test(entry.name) ? [path] : [];
  });
}

const srcRoot = fileURLToPath(new URL("../src", import.meta.url));
const frontend = sourceFiles(srcRoot).map((path) => readFileSync(path, "utf8")).join("\n") + html;

function parsePolicy(value, label) {
  assert.equal(typeof value, "string", `${label} must be an enabled CSP string`);
  const directives = new Map();
  for (const part of value.split(";")) {
    const [name, ...sources] = part.trim().split(/\s+/);
    if (!name) continue;
    assert.ok(!directives.has(name), `${label} contains duplicate ${name}`);
    directives.set(name, sources);
  }
  return directives;
}

function assertSources(policy, directive, expected) {
  assert.deepEqual(policy.get(directive), expected, `${directive} must use the reviewed source set`);
}

const security = config?.app?.security;
assert.ok(security, "Tauri app security config is required");
const production = parsePolicy(security.csp, "production CSP");
const development = parsePolicy(security.devCsp, "development CSP");

const restrictiveDirectives = {
  "default-src": ["'none'"],
  "script-src": ["'self'"],
  "img-src": ["'self'"],
  "font-src": ["'none'"],
  "base-uri": ["'none'"],
  "form-action": ["'none'"],
  "frame-ancestors": ["'none'"],
  "object-src": ["'none'"],
  "frame-src": ["'none'"],
  "media-src": ["'none'"],
  "worker-src": ["'none'"],
  "manifest-src": ["'none'"],
};
for (const [label, policy] of [["production", production], ["development", development]]) {
  for (const [directive, expected] of Object.entries(restrictiveDirectives)) {
    assertSources(policy, directive, expected);
  }
  assertSources(policy, "style-src", label === "development" ? ["'self'", "'unsafe-inline'"] : ["'self'"]);
  assertSources(
    policy,
    "connect-src",
    label === "development"
      ? ["ipc:", "http://ipc.localhost", "ws://localhost:5173"]
      : ["ipc:", "http://ipc.localhost"],
  );
  for (const [directive, sources] of policy) {
    for (const source of sources) {
      if (source.startsWith("'unsafe-")) {
        assert.ok(
          label === "development" && directive === "style-src" && source === "'unsafe-inline'",
          `${label} may not enable unsafe CSP sources except dev-only inline CSS for Vite HMR`,
        );
      }
    }
  }
}

assert.equal(security.devCsp !== undefined, true, "development allowances must be separated from production");
assert.notEqual(security.assetProtocol?.enable, true, "do not expose local files through asset protocol without a reviewed scope");
assert.doesNotMatch(frontend, /convertFileSrc\s*\(/, "frontend does not require file-system asset protocol access");
assert.doesNotMatch(frontend, /\bfetch\s*\(|\bXMLHttpRequest\b|\bWebSocket\b|\bEventSource\b|sendBeacon\s*\(/, "frontend network traffic must stay on reviewed Tauri IPC");
assert.doesNotMatch(frontend, /https?:\/\//, "frontend must not embed unreviewed remote origins");
assert.match(html, /<script\b[^>]*\bsrc=/i, "entrypoint script must be an external bundled module");
assert.doesNotMatch(html, /<script(?![^>]*\bsrc=)[^>]*>/i, "index.html must not contain inline scripts");
assert.doesNotMatch(html, /<style\b/i, "index.html must not contain inline style blocks");
assert.match(main, /import\s+["']\.\/index\.css["']/, "dev-only inline CSS allowance is for the Vite stylesheet import");
assert.match(viteConfig, /port:\s*5173/, "dev-only websocket source must match Vite's configured port");
assert.match(frontend, /app-icon\.png/, "self-hosted app icon remains available under img-src 'self'");
assert.doesNotMatch(frontend, /@font-face|url\(\s*['"]?https?:/, "no remote font or CSS URL source is needed");

console.log("PASS production WebView CSP denies unneeded sources; dev-only CSP allows IPC and Vite HMR");
