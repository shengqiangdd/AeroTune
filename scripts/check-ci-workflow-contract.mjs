import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const workflow = read(".github/workflows/ci.yml");
const dependabot = read(".github/dependabot.yml");
const packageJson = JSON.parse(read("package.json"));
const pnpmPolicy = read("pnpm-workspace.yaml");
assert.equal(
  existsSync(new URL("../package-lock.json", import.meta.url)),
  false,
  "pnpm-lock.yaml is the only supported root JavaScript lockfile",
);

const pinnedActions = [...workflow.matchAll(/^\s+uses:\s+([^@\s]+)@([a-f0-9]{40})\s+#\s+v[\w.-]+\s*$/gm)];
assert.equal(pinnedActions.length, 3, "all external actions must use full commit-SHA pins with reviewable version comments");
assert.doesNotMatch(workflow, /^\s+uses:\s+[^@\s]+@v\d+/m, "mutable action tags are not allowed");
assert.match(workflow, /runs-on:\s*windows-2025/);
assert.doesNotMatch(workflow, /runs-on:\s*windows-latest/);
assert.match(workflow, /toolchain:\s*1\.97\.1/);
assert.match(workflow, /components:\s*rustfmt, clippy/);
assert.match(workflow, /permissions:\s*\n\s+contents:\s*read/);
assert.doesNotMatch(workflow, /\$\{\{\s*secrets\./i, "validation CI must not receive signing or publishing secrets");
assert.match(workflow, /persist-credentials:\s*false/);
assert.match(workflow, /pnpm install --frozen-lockfile/);
assert.match(workflow, /pnpm run check:release/);
assert.match(workflow, /guarded-tauri\.mjs build --no-bundle/);
assert.match(packageJson.scripts["check:release"], /cargo test --manifest-path src-tauri\/Cargo\.toml --locked/);
assert.match(packageJson.scripts["check:release"], /vendor\/tauri-plugin-updater\/Cargo\.toml --locked/);
assert.match(packageJson.scripts["check:release"], /cargo clippy .*--all-targets .*clippy::correctness .*clippy::suspicious/);
assert.equal(packageJson.packageManager, "pnpm@11.8.0");
assert.match(pnpmPolicy, /esbuild:\s*true/);
assert.match(dependabot, /package-ecosystem:\s*github-actions/);
assert.match(dependabot, /package-ecosystem:\s*npm/);
assert.match(dependabot, /package-ecosystem:\s*cargo/);

console.log("PASS Windows CI is pinned, least-privilege, secret-free, reproducible, and release-gated");
