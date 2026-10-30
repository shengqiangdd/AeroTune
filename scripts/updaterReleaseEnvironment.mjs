import { frontendBuildEnvironment } from "./build-frontend-sanitized.mjs";

const RELEASE_ONLY_ENV_KEYS = new Set([
  "AEROTUNE_UPDATER_CONFIG",
  "AEROTUNE_UPDATE_ENDPOINT",
  "AEROTUNE_UPDATE_PUBLIC_KEY",
  "TAURI_CONFIG",
]);

export function updaterReleaseTestEnvironment(source = process.env) {
  const environment = frontendBuildEnvironment(source);
  for (const key of Object.keys(environment)) {
    if (RELEASE_ONLY_ENV_KEYS.has(key.toUpperCase())) delete environment[key];
  }
  return environment;
}
