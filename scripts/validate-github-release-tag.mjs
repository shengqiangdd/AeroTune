import { readFileSync } from "node:fs";

const tag = process.env.AEROTUNE_RELEASE_TAG?.trim();
const packageJson = JSON.parse(readFileSync("package.json", "utf8"));
const tauriConfig = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const cargoToml = readFileSync("src-tauri/Cargo.toml", "utf8");
const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!cargoVersion || packageJson.version !== cargoVersion || tauriConfig.version !== cargoVersion) {
  throw new Error("Cargo, package.json, and Tauri versions must match before release.");
}
if (tag !== `v${cargoVersion}`) {
  throw new Error(`Release tag must exactly match the synchronized version: v${cargoVersion}.`);
}
const taggedCommit = process.env.AEROTUNE_RELEASE_TAG_COMMIT?.trim();
const headCommit = process.env.AEROTUNE_CHECKED_OUT_COMMIT?.trim();
if (!taggedCommit || !headCommit || taggedCommit !== headCommit) {
  throw new Error("The release tag must point to the checked-out, reviewed commit.");
}
if (process.env.AEROTUNE_RELEASE_WORKTREE_CLEAN !== "true") {
  throw new Error("The release checkout must be clean; refusing to build or publish uncommitted source.");
}
console.log(`PASS release tag ${tag} matches clean commit ${headCommit}.`);
