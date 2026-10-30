import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { frontendBuildEnvironment, hasSigningMaterial, tauriBundleSigningEnvironment } from "./build-frontend-sanitized.mjs";

const args = process.argv.slice(2);
const isBuild = args[0] === "build";
const isBundle = args[0] === "bundle";
const isNoBundleBuild = isBuild && args.includes("--no-bundle");
const isPackagingCommand = isBuild || isBundle;
const configPaths = [];
for (let index = 0; index < args.length; index += 1) {
  const argument = args[index];
  if (argument === "--config" || argument === "-c") {
    const value = args[index + 1];
    if (!value || value.startsWith("-")) {
      console.error("FAIL: --config must be followed by one config file path.");
      process.exit(1);
    }
    configPaths.push(value);
    index += 1;
  } else if (argument.startsWith("--config=")) {
    configPaths.push(argument.slice("--config=".length));
  } else if (argument.startsWith("-c=")) {
    configPaths.push(argument.slice("-c=".length));
  }
}

const env = { ...process.env };
if (isPackagingCommand) {
  if (isBuild) {
    if (
      args.some(
        (argument) =>
          argument === "--runner" || argument.startsWith("--runner=") || argument.startsWith("-r"),
      )
    ) {
      console.error("FAIL: guarded Tauri builds do not permit a custom runner that can skip Cargo security checks.");
      process.exit(1);
    }
    if (!isNoBundleBuild && hasSigningMaterial(env)) {
      console.error("FAIL: updater signing secrets may enter Cargo build scripts; build with --no-bundle, then run the guarded bundle command.");
      process.exit(1);
    }
  }
  if (configPaths.length > 1) {
    console.error("FAIL: guarded Tauri packaging accepts exactly one config overlay.");
    process.exit(1);
  }
  const configPath = configPaths[0];
  if (configPath && /^[\s]*[\[{]/.test(configPath)) {
    console.error("FAIL: inline Tauri config cannot be audited; use a checked-in config file.");
    process.exit(1);
  }
  if (configPath) env.AEROTUNE_UPDATER_CONFIG = configPath;
  if (isBundle && hasSigningMaterial(env) && !env.AEROTUNE_UPDATER_CONFIG?.trim()) {
    console.error("FAIL: updater signing credentials may only be used with an audited AEROTUNE_UPDATER_CONFIG overlay.");
    process.exit(1);
  }

  // Direct Tauri packaging must not bypass the source-provenance and IPC allowlist gate.
  const provenancePreflight = spawnSync(
    process.execPath,
    [resolve("scripts/check-tauri-command-registration.mjs")],
    { stdio: "inherit", env: frontendBuildEnvironment(env) },
  );
  if (provenancePreflight.error) {
    console.error(`FAIL: source integrity preflight could not run (${provenancePreflight.error.message}).`);
    process.exit(1);
  }
  if (provenancePreflight.status !== 0) process.exit(provenancePreflight.status ?? 1);

  const preflight = spawnSync(process.execPath, [resolve("scripts/check-updater-release-ready.mjs")], {
    stdio: "inherit",
    env,
  });
  if (preflight.error) {
    console.error(`FAIL: updater preflight could not run (${preflight.error.message}).`);
    process.exit(1);
  }
  if (preflight.status !== 0) process.exit(preflight.status ?? 1);
}

const cli = resolve("node_modules/@tauri-apps/cli/tauri.js");
const cliEnv = isBundle ? tauriBundleSigningEnvironment(env) : frontendBuildEnvironment(env);
const result = spawnSync(process.execPath, [cli, ...args], { stdio: "inherit", env: cliEnv });
if (result.error) {
  console.error(`Tauri CLI failed to launch (${result.error.message}).`);
  process.exit(1);
}
process.exitCode = result.status ?? 1;
