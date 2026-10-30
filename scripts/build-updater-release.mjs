import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { updaterReleaseTestEnvironment } from "./updaterReleaseEnvironment.mjs";
import {
  createValidatedUpdaterOverlaySnapshot,
  removeValidatedUpdaterOverlaySnapshot,
} from "./updaterOverlaySnapshot.mjs";

const node = process.execPath;
const root = process.cwd();
const environment = { ...process.env };
const testEnvironment = updaterReleaseTestEnvironment(environment);
let overlaySnapshot;

function run(label, args, env) {
  const result = spawnSync(node, args.map((arg) => resolveFromRoot(arg)), {
    cwd: root,
    env,
    stdio: "inherit",
  });
  if (result.error) throw new Error(`${label} could not start (${result.error.message}).`);
  if (result.status !== 0) {
    const error = new Error(`${label} failed with exit code ${result.status ?? 1}.`);
    error.exitCode = result.status ?? 1;
    throw error;
  }
}

function resolveFromRoot(scriptPath) {
  return scriptPath.startsWith("scripts/") ? resolve(root, scriptPath) : scriptPath;
}

try {
  run("elevated task contract", ["scripts/check-elevated-test-task-contract.mjs"], testEnvironment);
  run("signed updater task contract", ["scripts/check-signed-updater-task-contract.mjs"], testEnvironment);
  run("updater release contracts", ["scripts/check-updater-release-contract.mjs"], testEnvironment);
  run("dynamic Cargo guard matrix", ["scripts/check-updater-build-guard.mjs"], testEnvironment);

  const sourceOverlayPath = environment.AEROTUNE_UPDATER_CONFIG?.trim();
  if (sourceOverlayPath) {
    overlaySnapshot = createValidatedUpdaterOverlaySnapshot(sourceOverlayPath);
    environment.AEROTUNE_UPDATER_CONFIG = overlaySnapshot.path;
  }

  run("required updater release preflight", ["scripts/check-updater-release-ready.mjs", "--required"], environment);
  run("Tauri compile without bundling", ["scripts/guarded-tauri.mjs", "build", "--no-bundle"], environment);

  const overlayPath = environment.AEROTUNE_UPDATER_CONFIG?.trim();
  if (!overlayPath) throw new Error("AEROTUNE_UPDATER_CONFIG must identify the reviewed bundle overlay.");
  run("signed Tauri bundle", ["scripts/guarded-tauri.mjs", "bundle", "--config", overlayPath], environment);
} catch (error) {
  if (typeof error?.exitCode === "number") {
    process.exitCode = error.exitCode;
  } else {
    console.error(`FAIL: ${error instanceof Error ? error.message : "updater release orchestration failed"}`);
    process.exitCode = 1;
  }
} finally {
  if (overlaySnapshot) {
    try {
      removeValidatedUpdaterOverlaySnapshot(overlaySnapshot);
    } catch (error) {
      console.error(`FAIL: updater overlay snapshot cleanup failed (${error instanceof Error ? error.message : "unknown error"}).`);
      process.exitCode = 1;
    }
  }
}
