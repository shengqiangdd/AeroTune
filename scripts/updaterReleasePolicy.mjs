import { lstatSync, readdirSync, readFileSync, realpathSync } from "node:fs";
import { isIP } from "node:net";
import { isAbsolute, join, relative, resolve, sep } from "node:path";

const RESERVED_HOSTS = new Set(["example.com", "example.org", "example.net", "localhost"]);
const RESERVED_DNS_SUFFIXES = [
  ".localhost",
  ".local",
  ".internal",
  ".test",
  ".invalid",
  ".example",
  ".example.com",
  ".example.org",
  ".example.net",
  ".onion",
  ".lan",
  ".corp",
  ".intranet",
  ".private",
  ".localdomain",
];
const UPDATER_PERMISSION = /\bupdater:[a-z0-9_-]+/i;
const MAX_UPDATER_RELEASE_CONFIG_BYTES = 64 * 1024;
const STRICT_BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

function decodeStrictBase64(value) {
  if (typeof value !== "string" || value.length === 0 || value.length % 4 !== 0 || !STRICT_BASE64.test(value)) {
    return null;
  }
  const decoded = Buffer.from(value, "base64");
  return decoded.toString("base64") === value ? decoded : null;
}

export function isValidTauriPublicKey(publicKey) {
  if (typeof publicKey !== "string" || publicKey.length > 4096) return false;
  const decoded = decodeStrictBase64(publicKey);
  if (!decoded) return false;
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(decoded);
  } catch {
    return false;
  }
  const [comment, keyMaterial] = text.split(/\r?\n/);
  if (!comment?.startsWith("untrusted comment:") || !keyMaterial) return false;
  const rawKey = decodeStrictBase64(keyMaterial);
  return rawKey !== null
    && rawKey.length === 42
    && rawKey[0] === 0x45
    && (rawKey[1] === 0x64 || rawKey[1] === 0x44);
}

export function readUpdaterReleaseConfigFile(configPath) {
  const path = resolve(String(configPath));
  const before = lstatSync(path);
  if (before.isSymbolicLink() || !before.isFile()) {
    throw new Error("Updater release config must be a regular, non-symlink file.");
  }
  if (before.size > MAX_UPDATER_RELEASE_CONFIG_BYTES) {
    throw new Error("Updater release config exceeds the 64 KiB review limit.");
  }
  const source = readFileSync(path);
  const after = lstatSync(path);
  if (
    after.isSymbolicLink()
    || !after.isFile()
    || before.dev !== after.dev
    || before.ino !== after.ino
    || before.size !== after.size
    || before.mtimeMs !== after.mtimeMs
    || source.byteLength !== before.size
    || source.byteLength > MAX_UPDATER_RELEASE_CONFIG_BYTES
  ) {
    throw new Error("Updater release config changed during validation or exceeds the 64 KiB review limit.");
  }
  let text;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(source);
  } catch {
    throw new Error("Updater release config must use valid UTF-8 encoding.");
  }
  let config;
  try {
    config = JSON.parse(text);
  } catch (error) {
    throw new Error(`Updater release config is invalid JSON: ${error instanceof Error ? error.message : "parse failed"}`);
  }
  if (!config || typeof config !== "object" || Array.isArray(config)) {
    throw new Error("Updater release config must be a JSON object.");
  }
  return config;
}

export function hasUpdaterPermission(value) {
  if (typeof value === "string") return UPDATER_PERMISSION.test(value);
  if (Array.isArray(value)) return value.some(hasUpdaterPermission);
  if (value && typeof value === "object") return Object.values(value).some(hasUpdaterPermission);
  return false;
}

export function readCapabilityFiles(directory) {
  const files = [];
  const visit = (current) => {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isSymbolicLink()) {
        throw new Error(`Capability tree contains a symbolic link: ${relative(directory, path)}`);
      }
      if (entry.isDirectory()) {
        visit(path);
      } else if (entry.isFile() && /\.(json|json5|toml)$/i.test(entry.name)) {
        files.push({ path: relative(directory, path), content: readFileSync(path, "utf8") });
      }
    }
  };
  visit(directory);
  return files;
}

function isPlaceholder(value) {
  const text = String(value ?? "").trim();
  const lower = text.toLowerCase();
  return !text || /replace|placeholder|your[\s_-]|example/i.test(text) || lower.includes("<") || lower.includes(">");
}

function normalizeHostname(hostname) {
  const unwrapped = hostname.startsWith("[") && hostname.endsWith("]")
    ? hostname.slice(1, -1)
    : hostname;
  return unwrapped.replace(/\.+$/, "").toLowerCase();
}

function isIpLiteralHostname(hostname) {
  return isIP(normalizeHostname(hostname)) !== 0;
}

function isLikelyKeyPath(value) {
  return isAbsolute(value)
    || /^[A-Za-z]:[\\/]/.test(value)
    || /^\\\\/.test(value)
    || /^\.{1,2}[\\/]/.test(value)
    || /\.(?:key|pem|priv)$/i.test(value);
}

function isInsideRepository(path) {
  let repositoryRoot;
  let canonicalPath;
  try {
    repositoryRoot = realpathSync(resolve(process.cwd()));
    canonicalPath = realpathSync(resolve(path));
  } catch {
    return true;
  }
  const relativePath = relative(repositoryRoot, canonicalPath);
  return !isAbsolute(relativePath)
    && relativePath !== ".."
    && !relativePath.startsWith(`..${sep}`);
}

function isNonEmptyKeyFile(value) {
  try {
    const path = resolve(value);
    const keyStats = lstatSync(path);
    return !keyStats.isSymbolicLink()
      && keyStats.isFile()
      && keyStats.size > 0
      && !isInsideRepository(path);
  } catch {
    return false;
  }
}

/** Rejects absent/placeholders, repository-local or symlinked files; Tauri parses/signs the input. */
export function hasPrivateKeyInput(value) {
  const text = String(value ?? "").trim();
  if (!text) return false;
  try {
    const path = resolve(text);
    const stats = lstatSync(path);
    return !stats.isSymbolicLink()
      && stats.isFile()
      && stats.size > 0
      && !isInsideRepository(path);
  } catch {
    return !isPlaceholder(text) && !isLikelyKeyPath(text);
  }
}

function nonEmptyEnvironmentValues(environment, expectedName) {
  return Object.entries(environment)
    .filter(([name]) => name.toUpperCase() === expectedName)
    .map(([, value]) => String(value ?? "").trim())
    .filter(Boolean);
}

export function hasConfiguredSigningKey(environment = {}) {
  const keyInputs = nonEmptyEnvironmentValues(environment, "TAURI_SIGNING_PRIVATE_KEY");
  const keyPaths = nonEmptyEnvironmentValues(environment, "TAURI_SIGNING_PRIVATE_KEY_PATH");
  if (keyInputs.length > 1 || keyPaths.length > 1 || (keyInputs.length > 0 && keyPaths.length > 0)) return false;
  if (keyInputs.length === 1) return hasPrivateKeyInput(keyInputs[0]);
  return keyPaths.length === 1 && isNonEmptyKeyFile(keyPaths[0]);
}

export function updaterIsEnabled(config) {
  return config?.bundle?.createUpdaterArtifacts === true && Boolean(config?.plugins?.updater);
}

/**
 * Validates an optional Tauri updater config overlay for AeroTune's protected per-machine install.
 * This only checks configuration prerequisites; it does not implement or prove the privileged runner.
 */
export function validateUpdaterReleaseConfig(baseConfig, overlay, {
  privateKeyPresent,
  signingKeyProtectionReady = false,
  runtimeEndpoint,
  runtimePublicKey,
  pipelineReady = false,
  feedResponseBoundedReady = false,
  dnsDestinationPolicyReady = false,
  proxyPolicyReady = false,
  boundedDownloadReady = false,
  elevatedRunnerReady = false,
  capabilityFiles = [],
} = {}) {
  const errors = [];
  const warnings = [];
  const updater = overlay?.plugins?.updater;
  const baseVersion = baseConfig?.version;
  const strictSemverPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
  if (baseConfig?.productName !== "AeroTune") {
    errors.push("The base productName must remain AeroTune because Tauri derives the Windows install folder and installer names from it.");
  }
  if (baseConfig?.identifier !== "com.aerotune.desktop") {
    errors.push("The base identifier must remain com.aerotune.desktop for the fixed AeroTune app identity.");
  }
  if (typeof baseVersion !== "string" || !strictSemverPattern.test(baseVersion)) {
    errors.push("The base version must be a strict SemVer string used by the signed updater.");
  }
  if (baseConfig?.build?.beforeBuildCommand !== "node scripts/build-frontend-sanitized.mjs") {
    errors.push("The base beforeBuildCommand must use the sanitized frontend wrapper so npm build scripts never inherit updater signing credentials.");
  }
  const baseBuild = baseConfig?.build ?? {};
  if (baseBuild.beforeBundleCommand != null || Object.hasOwn(baseBuild, "before-bundle-command")) {
    errors.push("The base beforeBundleCommand must remain unset because it would run with updater signing credentials.");
  }
  const identityAliases = ["product-name", "main-binary-name"];
  for (const [config, label] of [[baseConfig, "base"], [overlay, "release overlay"]]) {
    if (!config || typeof config !== "object" || Array.isArray(config)) continue;
    for (const alias of identityAliases) {
      if (Object.hasOwn(config, alias)) {
        errors.push(`${label} app identity must not use serde alias ${alias}; use audited canonical keys.`);
      }
    }
    if (Object.hasOwn(config, "mainBinaryName")) {
      errors.push(`${label} must not override mainBinaryName; the fixed AeroTune executable is required by the protected task.`);
    }
    for (const key of ["productName", "identifier", "version"]) {
      if (label === "release overlay" && Object.hasOwn(config, key) && config[key] !== baseConfig?.[key]) {
        errors.push(`${label} must not override ${key}; it is bound to the protected AeroTune installation identity.`);
      }
    }
  }
  const allowedOverlayKeys = new Set(["$schema", "bundle", "plugins"]);
  if (!overlay || typeof overlay !== "object" || Array.isArray(overlay)) {
    errors.push("The signed updater release overlay must be a JSON object.");
  } else {
    for (const key of Object.keys(overlay)) {
      if (!allowedOverlayKeys.has(key)) {
        errors.push(`Release overlay root key ${key} is forbidden; only $schema, bundle, and plugins are allowed so app and build commands stay fixed.`);
      }
    }
    const bundleKeys = new Set(["createUpdaterArtifacts", "targets", "windows"]);
    for (const key of Object.keys(overlay.bundle ?? {})) {
      if (!bundleKeys.has(key)) {
        errors.push(`Release overlay bundle key ${key} is outside the reviewed updater configuration scope.`);
      }
    }
    const pluginKeys = new Set(["updater"]);
    for (const key of Object.keys(overlay.plugins ?? {})) {
      if (!pluginKeys.has(key)) {
        errors.push(`Release overlay plugin ${key} is outside the reviewed updater configuration scope.`);
      }
    }
  }
  const overlayWindows = overlay?.bundle?.windows;
  if (overlayWindows && typeof overlayWindows === "object" && !Array.isArray(overlayWindows)) {
    for (const key of Object.keys(overlayWindows)) {
      if (!new Set(["allowDowngrades", "allow-downgrades", "nsis"]).has(key)) {
        errors.push(`Release overlay bundle.windows key ${key} is outside the reviewed updater scope.`);
      }
    }
    const overlayNsis = overlayWindows.nsis;
    if (overlayNsis && typeof overlayNsis === "object" && !Array.isArray(overlayNsis)) {
      const reviewedNsisKeys = new Set([
        "installMode",
        "install-mode",
        "installer-hooks",
        "template",
        "installerHooks",
        "customLanguageFiles",
      ]);
      for (const key of Object.keys(overlayNsis)) {
        if (!reviewedNsisKeys.has(key)) {
          errors.push(`Release overlay bundle.windows.nsis key ${key} is outside the reviewed NSIS scope.`);
        }
      }
    }
  }
  if (updater !== undefined) {
    if (!updater || typeof updater !== "object" || Array.isArray(updater)) {
      errors.push("Release overlay plugins.updater must be an object.");
    } else {
      const reviewedUpdaterKeys = new Set([
        "pubkey",
        "endpoints",
        "requireSignedVersion",
        "require-signed-version",
        "allowDowngrades",
        "allow-downgrades",
        "dangerousInsecureTransportProtocol",
        "dangerous-insecure-transport-protocol",
        "dangerousAcceptInvalidCerts",
        "dangerous-accept-invalid-certs",
        "dangerousAcceptInvalidHostnames",
        "dangerous-accept-invalid-hostnames",
        "noProxy",
        "no_proxy",
        "proxy",
        "windows",
      ]);
      for (const key of Object.keys(updater)) {
        if (!reviewedUpdaterKeys.has(key)) {
          errors.push(`Release overlay plugins.updater key ${key} is outside the reviewed updater configuration scope.`);
        }
      }
      const updaterWindows = updater.windows;
      if (updaterWindows && typeof updaterWindows === "object" && !Array.isArray(updaterWindows)) {
        const reviewedUpdaterWindowsKeys = new Set([
          "installMode",
          "install-mode",
          "installerArgs",
          "installer-args",
        ]);
        for (const key of Object.keys(updaterWindows)) {
          if (!reviewedUpdaterWindowsKeys.has(key)) {
            errors.push(`Release overlay plugins.updater.windows key ${key} is outside the reviewed updater Windows scope.`);
          }
        }
      }
    }
  }

  const updaterAliases = [
    "dangerous-insecure-transport-protocol",
    "dangerous-accept-invalid-certs",
    "dangerous-accept-invalid-hostnames",
    "require-signed-version",
    "allow-downgrades",
  ];
  for (const [config, label] of [[baseConfig?.plugins?.updater, "base"], [updater, "release overlay"]]) {
    if (!config || typeof config !== "object" || Array.isArray(config)) continue;
    for (const alias of updaterAliases) {
      if (Object.hasOwn(config, alias)) {
        errors.push(`${label} plugins.updater must not use serde alias ${alias}; use the canonical audited key.`);
      }
    }
  }

  if (Object.hasOwn(overlay?.app?.security ?? {}, "capabilities")) {
    errors.push("The updater release overlay must not override app.security.capabilities; capability access is audited separately.");
  }
  if (hasUpdaterPermission(baseConfig?.app?.security?.capabilities)) {
    errors.push("The base app.security.capabilities must not grant updater plugin commands.");
  }
  for (const capability of capabilityFiles) {
    const content = typeof capability === "string" ? capability : capability?.content;
    if (hasUpdaterPermission(content)) {
      errors.push(`Updater plugin permissions are forbidden in capability file ${capability?.path ?? "(unknown)"}.`);
    }
  }

  if (!updaterIsEnabled(overlay)) errors.push("Updater artifacts and plugins.updater must both be configured in the release overlay.");
  if (!privateKeyPresent) errors.push("Configure exactly one non-placeholder TAURI_SIGNING_PRIVATE_KEY input (inline secret or a non-empty regular non-symlink file outside the repository) or a TAURI_SIGNING_PRIVATE_KEY_PATH file with those same file requirements; signed updater artifacts cannot be produced otherwise.");
  if (!signingKeyProtectionReady) errors.push("Private signing-key secret-store/file ACL protection has not been audited; updater release remains disabled.");

  const publicKey = String(updater?.pubkey ?? "").trim();
  if (isPlaceholder(publicKey) || /PRIVATE KEY/i.test(publicKey)) {
    errors.push("plugins.updater.pubkey must contain only the real public key; placeholders/private keys are rejected.");
  } else if (!isValidTauriPublicKey(publicKey)) {
    errors.push("plugins.updater.pubkey must be a valid Tauri Minisign public key encoded as base64.");
  }

  if (updater?.requireSignedVersion !== true) {
    errors.push("plugins.updater.requireSignedVersion must be true to bind release metadata to its signed artifact.");
  }
  if (updater?.allowDowngrades !== false) {
    errors.push("plugins.updater.allowDowngrades must be false; update versions must progress strictly forward.");
  }

  const endpoints = updater?.endpoints;
  if (!Array.isArray(endpoints) || endpoints.length === 0) {
    errors.push("At least one fixed HTTPS updater endpoint is required.");
  } else {
    for (const endpoint of endpoints) {
      try {
        const parsed = new URL(endpoint);
        const rawHostname = parsed.hostname;
        const hostname = normalizeHostname(rawHostname);
        const labels = hostname.split(".");
        if (parsed.protocol !== "https:") errors.push("Updater endpoint must use HTTPS.");
        if (isIpLiteralHostname(hostname)) errors.push("Updater endpoints must use a DNS hostname, not an IP literal.");
        if (parsed.username || parsed.password) errors.push("Updater endpoints must not contain embedded credentials.");
        if (parsed.port) errors.push("Updater endpoints must use the default HTTPS port only.");
        if (parsed.search || parsed.hash) errors.push("Updater endpoints must not contain query strings or fragments.");
        if (rawHostname.endsWith(".")) errors.push("Updater endpoint hostname must not have a trailing dot.");
        if (labels.length < 2 || labels.some((label) =>
          label.length > 63 || !/^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(label),
        )) {
          errors.push("Updater endpoint must use a fully-qualified DNS hostname with valid labels.");
        }
        if (RESERVED_HOSTS.has(hostname)
          || RESERVED_DNS_SUFFIXES.some((suffix) => hostname.endsWith(suffix))) {
          errors.push(`Reserved/example updater host is not a production release feed: ${parsed.hostname}`);
        }
      } catch {
        errors.push("Updater endpoint is not a valid URL.");
      }
    }
  }

  if (Array.isArray(endpoints)) {
    if (endpoints.length !== 1) errors.push("The current protected runner supports exactly one fixed update endpoint.");
    if (runtimeEndpoint !== endpoints[0]) errors.push("AEROTUNE_UPDATE_ENDPOINT must exactly match the release overlay endpoint.");
  }
  if (runtimePublicKey !== publicKey) errors.push("AEROTUNE_UPDATE_PUBLIC_KEY must exactly match plugins.updater.pubkey.");
  if (!pipelineReady) errors.push("The code-level updater gate remains disabled until rollback and privileged validation are complete.");
  if (!feedResponseBoundedReady) errors.push("The vendored 256 KiB feed-manifest cap is a prototype and has not passed release review or feed-size measurement; updater release remains disabled.");
  if (!dnsDestinationPolicyReady) errors.push("Direct-route DNS destination pinning or a reviewed system-proxy egress allowlist/rebinding defense is not implemented; updater release remains disabled.");
  if (!proxyPolicyReady) errors.push("The selected policy preserves Windows system proxy, but its routing, origin-DNS behavior, and fail-closed fallback have not passed disposable-VM validation; updater release remains disabled.");
  if (!boundedDownloadReady) errors.push("Reviewed artifact byte caps, a release-ready bounded downloader, exact-byte signature verification, and protected staging are not complete; updater release remains disabled.");

  if (updater && Object.hasOwn(updater, "dangerousInsecureTransportProtocol")
    && updater.dangerousInsecureTransportProtocol !== false) {
    errors.push("dangerousInsecureTransportProtocol must remain disabled.");
  }
  if (updater && ["dangerousAcceptInvalidCerts", "dangerousAcceptInvalidHostnames"].some(
    (option) => Object.hasOwn(updater, option) && updater[option] !== false,
  )) {
    errors.push("dangerousAcceptInvalidCerts and dangerousAcceptInvalidHostnames must remain disabled.");
  }
  if (updater && ["noProxy", "no_proxy"].some(
    (option) => Object.hasOwn(updater, option) && updater[option] !== false,
  )) {
    errors.push("The selected Windows system-proxy policy forbids noProxy overrides.");
  }
  if (updater?.proxy !== undefined && updater?.proxy !== null) {
    errors.push("Explicit updater proxy overrides require a separately reviewed proxy policy.");
  }
  const windows = updater?.windows;
  if (windows !== undefined && (!windows || typeof windows !== "object" || Array.isArray(windows))) {
    errors.push("plugins.updater.windows must be an object when overridden.");
  } else if (windows) {
    for (const alias of ["installer-args", "install-mode"]) {
      if (Object.hasOwn(windows, alias)) {
        errors.push(`plugins.updater.windows must not use serde alias ${alias}; use the canonical audited key.`);
      }
    }
    if (Object.hasOwn(windows, "installerArgs")
      && (!Array.isArray(windows.installerArgs) || windows.installerArgs.length !== 0)) {
      errors.push("plugins.updater.windows.installerArgs must remain an empty array to protect the fixed Program Files target.");
    }
    if (Object.hasOwn(windows, "installMode") && windows.installMode !== "quiet") {
      errors.push("plugins.updater.windows.installMode must remain quiet for the already-elevated fixed runner.");
    }
  }

  for (const [bundleConfig, label] of [
    [baseConfig?.bundle, "base"],
    [overlay?.bundle, "release overlay"],
  ]) {
    if (bundleConfig === undefined) continue;
    if (!bundleConfig || typeof bundleConfig !== "object" || Array.isArray(bundleConfig)) {
      errors.push(`${label} bundle must be an object when overridden.`);
      continue;
    }
    const windowsConfig = bundleConfig.windows;
    if (windowsConfig === undefined) continue;
    if (!windowsConfig || typeof windowsConfig !== "object" || Array.isArray(windowsConfig)) {
      errors.push(`${label} bundle.windows must be an object when overridden.`);
      continue;
    }
    if (Object.hasOwn(windowsConfig, "sign-command")) {
      errors.push(`${label} bundle.windows must not use serde alias sign-command; use signCommand.`);
    }
    if (windowsConfig.signCommand !== undefined && windowsConfig.signCommand !== null) {
      errors.push(`${label} bundle.windows.signCommand must remain unset; custom code-sign commands are not audited.`);
    }
    if (Object.hasOwn(windowsConfig, "allow-downgrades")) {
      errors.push(`${label} bundle.windows must not use serde alias allow-downgrades; use allowDowngrades.`);
    }
  }
  const bundleAllowDowngrades = baseConfig?.bundle?.windows?.allowDowngrades;
  if (bundleAllowDowngrades !== false) {
    errors.push("bundle.windows.allowDowngrades must be false; rollback must use a separate protected restore path.");
  }
  const overlayBundleAllowDowngrades = overlay?.bundle?.windows?.allowDowngrades;
  if (overlayBundleAllowDowngrades !== undefined && overlayBundleAllowDowngrades !== false) {
    errors.push("The release overlay must not enable bundle.windows.allowDowngrades; old installers are not a rollback mechanism.");
  }

  const baseNsis = baseConfig?.bundle?.windows?.nsis;
  const overlayNsis = overlay?.bundle?.windows?.nsis;
  for (const [nsis, label] of [[baseNsis, "base"], [overlayNsis, "release overlay"]]) {
    if (nsis === undefined) continue;
    if (!nsis || typeof nsis !== "object" || Array.isArray(nsis)) {
      errors.push(`${label} bundle.windows.nsis must be an object when configured.`);
      continue;
    }
    if (Object.hasOwn(nsis, "install-mode")) {
      errors.push(`${label} bundle.windows.nsis must not use serde alias install-mode; use installMode.`);
    }
    if (Object.hasOwn(nsis, "installer-hooks")) {
      errors.push(`${label} bundle.windows.nsis must not use serde alias installer-hooks.`);
    }
    if (Object.hasOwn(nsis, "install-dir")) {
      errors.push(`${label} bundle.windows.nsis must not use serde alias install-dir.`);
    }
    if (nsis.installDir !== undefined && nsis.installDir !== null) {
      errors.push(`${label} bundle.windows.nsis.installDir must remain unset to preserve Program Files\\AeroTune.`);
    }
    if (Object.hasOwn(nsis, "template") && nsis.template !== null) {
      errors.push(`${label} bundle.windows.nsis.template must remain unset to preserve the protected installer path.`);
    }
    if (Object.hasOwn(nsis, "installerHooks") && nsis.installerHooks !== null) {
      errors.push(`${label} bundle.windows.nsis.installerHooks must remain unset; custom hooks execute elevated installer script.`);
    }
    if (Object.hasOwn(nsis, "customLanguageFiles") && nsis.customLanguageFiles !== null) {
      errors.push(`${label} bundle.windows.nsis.customLanguageFiles must remain unset; custom NSIS source is not audited.`);
    }
    if (nsis.installMode !== "perMachine") {
      errors.push(label === "base"
        ? "The protected AeroTune updater design requires the base NSIS installMode to remain perMachine."
        : "Any release NSIS overlay must explicitly preserve installMode perMachine.");
    }
  }
  const installMode = baseNsis?.installMode;
  const baseTargets = baseConfig?.bundle?.targets;
  if (!Array.isArray(baseTargets) || baseTargets.length !== 1 || baseTargets[0] !== "nsis") {
    errors.push("The base bundle.targets must be exactly [\"nsis\"] for the protected updater installer.");
  }
  const overlayTargets = overlay?.bundle?.targets;
  if (!Array.isArray(overlayTargets) || overlayTargets.length !== 1 || overlayTargets[0] !== "nsis") {
    errors.push("The protected updater release overlay must explicitly target only the NSIS installer.");
  }
  if (updater?.windows?.installMode !== "quiet") {
    errors.push("A per-machine update runner must use quiet installer mode only after it is already elevated.");
  }
  if (!elevatedRunnerReady) {
    errors.push("The protected runner/task, rollback path, and privileged Windows validation are not all complete and audited yet.");
  }
  if (installMode === "perMachine" && updater?.windows?.installMode === "passive") {
    warnings.push("passive only changes UI; a standard-user per-machine update still needs UAC.");
  }

  return { enabled: updaterIsEnabled(overlay), errors, warnings };
}
