import { readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, resolve, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = dirname(fileURLToPath(import.meta.url));
const repositoryRoot = resolve(scriptDirectory, "..");
const checks = readdirSync(scriptDirectory)
  .filter((name) => /^check-[a-z0-9-]+\.mjs$/i.test(name))
  .sort();

if (checks.length === 0) {
  console.error("No check-*.mjs contract scripts were found.");
  process.exit(1);
}

for (const check of checks) {
  console.log(`\n=== ${check} ===`);
  const result = spawnSync(process.execPath, [join(scriptDirectory, check)], {
    cwd: repositoryRoot,
    env: process.env,
    stdio: "inherit",
  });
  if (result.error) {
    console.error(`${check} could not start: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`${check} failed with exit code ${result.status ?? "unknown"}.`);
    process.exit(result.status ?? 1);
  }
}

console.log(`\nAll ${checks.length} project contract checks passed.`);
