import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SIGNING_SECRET_ENV_KEYS = new Set([
  "TAURI_SIGNING_PRIVATE_KEY",
  "TAURI_SIGNING_PRIVATE_KEY_PATH",
  "TAURI_SIGNING_PRIVATE_KEY_PASSWORD",
]);

export function hasSigningMaterial(source = process.env) {
  return Object.entries(source).some(([key, value]) =>
    SIGNING_SECRET_ENV_KEYS.has(key.toUpperCase()) && typeof value === "string" && value.trim().length > 0,
  );
}

export function tauriBundleSigningEnvironment(source = process.env) {
  const entries = Object.entries(source);
  const keyInput = entries.find(([key]) => key.toUpperCase() === "TAURI_SIGNING_PRIVATE_KEY")?.[1];
  const keyPath = entries.find(([key]) => key.toUpperCase() === "TAURI_SIGNING_PRIVATE_KEY_PATH")?.[1];
  const password = entries.find(([key]) => key.toUpperCase() === "TAURI_SIGNING_PRIVATE_KEY_PASSWORD")?.[1];
  const environment = { ...source };
  for (const key of Object.keys(environment)) {
    if (SIGNING_SECRET_ENV_KEYS.has(key.toUpperCase())) delete environment[key];
  }
  const signingInput = typeof keyInput === "string" && keyInput.trim().length > 0
    ? keyInput
    : typeof keyPath === "string" && keyPath.trim().length > 0
      ? keyPath.trim()
      : undefined;
  if (signingInput !== undefined) environment.TAURI_SIGNING_PRIVATE_KEY = signingInput;
  if (typeof password === "string" && password.length > 0) environment.TAURI_SIGNING_PRIVATE_KEY_PASSWORD = password;
  return environment;
}

export function frontendBuildEnvironment(source = process.env) {
  const environment = { ...source };
  for (const key of Object.keys(environment)) {
    if (SIGNING_SECRET_ENV_KEYS.has(key.toUpperCase())) delete environment[key];
  }
  return environment;
}

export function runFrontendBuild(source = process.env) {
  const result = spawnSync("npm run build", {
    shell: true,
    stdio: "inherit",
    env: frontendBuildEnvironment(source),
  });
  if (result.error) {
    console.error(`Frontend build could not start (${result.error.message}).`);
    return 1;
  }
  return result.status ?? 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  process.exitCode = runFrontendBuild();
}
