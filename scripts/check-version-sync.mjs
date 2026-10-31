import assert from "node:assert/strict";
import fs from "node:fs";

const packageJson = JSON.parse(fs.readFileSync("package.json", "utf8"));
const pnpmLock = fs.readFileSync("pnpm-lock.yaml", "utf8");
const tauri = JSON.parse(fs.readFileSync("src-tauri/tauri.conf.json", "utf8"));
const cargoToml = fs.readFileSync("src-tauri/Cargo.toml", "utf8");
const cargoLock = fs.readFileSync("src-tauri/Cargo.lock", "utf8");
const cargoVersion = /^version\s*=\s*"([^"]+)"/m.exec(cargoToml)?.[1];
const lockedCargoVersion = /\[\[package\]\]\s*name\s*=\s*"aerotune"\s*version\s*=\s*"([^"]+)"/m.exec(cargoLock)?.[1];
const versions = {
  package: packageJson.version,
  tauri: tauri.version,
  cargo: cargoVersion,
  cargoLock: lockedCargoVersion,
};
assert.equal(packageJson.packageManager, "pnpm@11.8.0", "the supported JS package manager must stay pinned");
assert.match(pnpmLock, /^lockfileVersion:\s*'9\.0'$/m, "the canonical pnpm lockfile must be present");
assert.ok(Object.values(versions).every(Boolean), `could not read every version: ${JSON.stringify(versions)}`);
assert.equal(new Set(Object.values(versions)).size, 1, `version metadata differs: ${JSON.stringify(versions)}`);
console.log(`PASS synchronized app version ${packageJson.version} across the pnpm manifest, Tauri, and Cargo metadata`);
