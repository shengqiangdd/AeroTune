import assert from "node:assert/strict";
import { closeSync, mkdtempSync, openSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { updaterReleaseTestEnvironment } from "./updaterReleaseEnvironment.mjs";

const cargoManifest = readFileSync("src-tauri/Cargo.toml", "utf8");
const guardedTauri = readFileSync("scripts/guarded-tauri.mjs", "utf8");
const commandRegistration = readFileSync("scripts/check-tauri-command-registration.mjs", "utf8");
assert.match(commandRegistration, /Documented checkpoint size:/);
assert.match(commandRegistration, /Current src-tauri\/src\/lib\.rs size:/);
const provenanceGateAt = guardedTauri.indexOf('resolve("scripts/check-tauri-command-registration.mjs")');
const tauriLaunchAt = guardedTauri.indexOf('const cli = resolve("node_modules/@tauri-apps/cli/tauri.js")');
assert.ok(
  provenanceGateAt >= 0 && tauriLaunchAt > provenanceGateAt,
  "guarded packaging must run the provenance/IPC gate before invoking the Tauri CLI",
);
assert.match(
  guardedTauri,
  /if \(provenancePreflight\.status !== 0\) process\.exit\(provenancePreflight\.status \?\? 1\)/,
  "a failed provenance check must stop Tauri packaging",
);
const provenancePreflightStart = guardedTauri.indexOf("const provenancePreflight = spawnSync(");
const provenancePreflightEnd = guardedTauri.indexOf("if (provenancePreflight.error)", provenancePreflightStart);
assert.ok(provenancePreflightStart >= 0 && provenancePreflightEnd > provenancePreflightStart);
assert.match(
  guardedTauri.slice(provenancePreflightStart, provenancePreflightEnd),
  /env: frontendBuildEnvironment\(env\)/,
  "the read-only source contract must not receive Tauri signing secrets",
);
console.log("PASS guarded packaging enforces sanitized provenance/IPC preflight before Tauri");
const cargoVersion = cargoManifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
assert.ok(cargoVersion, "Cargo package version must be readable for version-override tests");
const versionOverride = cargoVersion === "0.0.1" ? "0.0.2" : "0.0.1";

const cases = [
  {
    label: "productName install-directory override",
    config: { productName: "OtherProduct" },
    expected: "must keep productName fixed to AeroTune",
  },
  {
    label: "product-name serde alias",
    config: { "product-name": "AeroTune" },
    expected: "accepted serde aliases are forbidden",
  },
  {
    label: "application identifier override",
    config: { identifier: "com.attacker.other" },
    expected: "must keep identifier fixed to com.aerotune.desktop",
  },
  {
    label: "signed app version override",
    config: { version: versionOverride },
    expected: `must keep version fixed to ${cargoVersion}`,
  },
  {
    label: "main binary name override",
    config: { mainBinaryName: "OtherApp" },
    expected: "must not override mainBinaryName",
  },
  {
    label: "main-binary-name serde alias",
    config: { "main-binary-name": "AeroTune" },
    expected: "accepted serde aliases are forbidden",
  },
  {
    label: "frontend build command override",
    config: { build: { beforeBuildCommand: "node malicious-build-hook.mjs" } },
    expected: "root key build could alter the protected app or build pipeline",
  },
  {
    label: "remote app window override",
    config: { app: { windows: [{ url: "https://attacker.example" }] } },
    expected: "root key app could alter the protected app or build pipeline",
  },
  {
    label: "unrelated plugin override",
    config: { plugins: { notification: { enabled: false } } },
    expected: "plugin notification is outside the reviewed updater release overlay scope",
  },
  {
    label: "custom Windows code-sign command",
    config: { bundle: { windows: { signCommand: "cmd.exe /c echo signing" } } },
    expected: "bundle.windows key signCommand is outside the reviewed updater release overlay scope",
  },
  {
    label: "Windows code-sign command alias",
    config: { bundle: { windows: { "sign-command": "cmd.exe /c echo signing" } } },
    expected: "bundle.windows key sign-command is outside the reviewed updater release overlay scope",
  },
  {
    label: "NSIS install-directory override",
    config: { bundle: { windows: { nsis: { installMode: "perMachine", installDir: "$PROGRAMFILES64\\Other" } } } },
    expected: "bundle.windows.nsis key installDir is outside the reviewed NSIS overlay scope",
  },
  {
    label: "NSIS install-directory alias",
    config: { bundle: { windows: { nsis: { "install-dir": "$PROGRAMFILES64\\Other" } } } },
    expected: "bundle.windows.nsis key install-dir is outside the reviewed NSIS overlay scope",
  },
  {
    label: "unreviewed updater headers",
    config: { plugins: { updater: { headers: { Authorization: "Bearer test-only" } } } },
    expected: "plugins.updater key headers is outside the reviewed updater configuration scope",
  },
  {
    label: "require-signed-version alias",
    config: { plugins: { updater: { "require-signed-version": false } } },
    expected: "serde alias require-signed-version is forbidden",
  },
  {
    label: "dangerous transport alias",
    config: { plugins: { updater: { "dangerous-insecure-transport-protocol": true } } },
    expected: "serde alias dangerous-insecure-transport-protocol is forbidden",
  },
  {
    label: "updater installer arguments",
    config: { plugins: { updater: { windows: { installerArgs: ["/D=C:\\Temp"] } } } },
    expected: "windows.installerArgs an empty array",
  },
  {
    label: "bundle downgrade alias",
    config: { bundle: { windows: { "allow-downgrades": true } } },
    expected: "bundle.windows.allowDowngrades key; the serde alias is forbidden",
  },
  {
    label: "non-NSIS bundle target",
    config: { bundle: { targets: ["msi"] } },
    expected: "bundle.targets must remain exactly",
  },
  {
    label: "all bundle targets",
    config: { bundle: { targets: "all" } },
    expected: "bundle.targets must remain exactly",
  },
  {
    label: "multiple bundle targets",
    config: { bundle: { targets: ["nsis", "msi"] } },
    expected: "bundle.targets must remain exactly",
  },
  {
    label: "NSIS install-mode alias",
    config: { bundle: { windows: { nsis: { "install-mode": "currentUser" } } } },
    expected: "canonical NSIS keys; serde aliases are forbidden",
  },
  {
    label: "non-per-machine NSIS mode",
    config: { bundle: { windows: { nsis: { installMode: "currentUser" } } } },
    expected: "nsis.installMode perMachine",
  },
  {
    label: "custom NSIS template",
    config: { bundle: { windows: { nsis: { installMode: "perMachine", template: "unsafe.nsi" } } } },
    expected: "custom NSIS template",
  },
  {
    label: "custom elevated NSIS hooks",
    config: { bundle: { windows: { nsis: { installMode: "perMachine", installerHooks: "custom.nsh" } } } },
    expected: "custom NSIS installer hooks with elevated script execution",
  },
  {
    label: "custom NSIS language source",
    config: { bundle: { windows: { nsis: { installMode: "perMachine", customLanguageFiles: { English: "custom.nsh" } } } } },
    expected: "custom NSIS language source files",
  },
];

const testDirectory = mkdtempSync(join(tmpdir(), "aerotune-updater-build-guard-"));
const cargoArgs = ["check", "--manifest-path", "src-tauri/Cargo.toml", "--locked"];

function runCargoCheck(environment, outputPath) {
  const cargoEnvironment = updaterReleaseTestEnvironment(environment);
  if (Object.hasOwn(environment, "TAURI_CONFIG")) {
    cargoEnvironment.TAURI_CONFIG = environment.TAURI_CONFIG;
  }
  const outputFd = openSync(outputPath, "w");
  let result;
  try {
    result = spawnSync("cargo", cargoArgs, {
      cwd: process.cwd(),
      env: cargoEnvironment,
      stdio: ["ignore", outputFd, outputFd],
    });
  } finally {
    closeSync(outputFd);
  }
  if (result.error) throw result.error;
  return { status: result.status, output: readFileSync(outputPath, "utf8") };
}

try {
  for (const [index, testCase] of cases.entries()) {
    const result = runCargoCheck(
      { ...process.env, TAURI_CONFIG: JSON.stringify(testCase.config) },
      join(testDirectory, `${index}.log`),
    );
    assert.notEqual(result.status, 0, `Cargo build guard accepted ${testCase.label}.`);
    assert.ok(
      result.output.includes(testCase.expected),
      `Cargo rejected ${testCase.label} for the wrong reason:\n${result.output}`,
    );
    console.log(`PASS build guard rejects ${testCase.label}`);
  }

  const signingCliEnvironment = updaterReleaseTestEnvironment(process.env);
  signingCliEnvironment.TAURI_SIGNING_PRIVATE_KEY = "dummy-secret-must-not-be-logged";
  const signingGuardLog = join(testDirectory, "guarded-tauri-signing.log");
  const signingGuardOutputFd = openSync(signingGuardLog, "w");
  let signingGuardResult;
  try {
    signingGuardResult = spawnSync(process.execPath, ["scripts/guarded-tauri.mjs", "build"], {
      cwd: process.cwd(),
      env: signingCliEnvironment,
      stdio: ["ignore", signingGuardOutputFd, signingGuardOutputFd],
    });
  } finally {
    closeSync(signingGuardOutputFd);
  }
  if (signingGuardResult.error) throw signingGuardResult.error;
  assert.equal(signingGuardResult.status, 1, "guarded one-step build must refuse to pass signing secrets into Cargo");
  const signingGuardOutput = readFileSync(signingGuardLog, "utf8");
  assert.ok(signingGuardOutput.includes("updater signing secrets may enter Cargo build scripts"));
  assert.ok(!signingGuardOutput.includes("dummy-secret-must-not-be-logged"));
  console.log("PASS guarded one-step build refuses signing secrets before Tauri/Cargo launch");

  const bundleGuardLog = join(testDirectory, "guarded-tauri-bundle.log");
  const bundleGuardOutputFd = openSync(bundleGuardLog, "w");
  let bundleGuardResult;
  try {
    bundleGuardResult = spawnSync(process.execPath, ["scripts/guarded-tauri.mjs", "bundle"], {
      cwd: process.cwd(),
      env: signingCliEnvironment,
      stdio: ["ignore", bundleGuardOutputFd, bundleGuardOutputFd],
    });
  } finally {
    closeSync(bundleGuardOutputFd);
  }
  if (bundleGuardResult.error) throw bundleGuardResult.error;
  assert.equal(bundleGuardResult.status, 1, "signed bundle without audited updater config must be refused");
  assert.ok(readFileSync(bundleGuardLog, "utf8").includes("only be used with an audited AEROTUNE_UPDATER_CONFIG overlay"));
  console.log("PASS signed bundle refuses to run without the audited updater overlay");

  const cleanEnvironment = { ...process.env };
  delete cleanEnvironment.TAURI_CONFIG;
  const cleanResult = runCargoCheck(cleanEnvironment, join(testDirectory, "clean.log"));
  assert.equal(cleanResult.status, 0, `Ordinary Cargo check failed:\n${cleanResult.output}`);
  console.log("PASS ordinary Cargo check passes without injected overlay");
} finally {
  const resolvedTemporaryDirectory = resolve(testDirectory);
  assert.equal(dirname(resolvedTemporaryDirectory), resolve(tmpdir()), "refusing to remove an unexpected temporary path");
  rmSync(resolvedTemporaryDirectory, { recursive: true, force: true });
}
