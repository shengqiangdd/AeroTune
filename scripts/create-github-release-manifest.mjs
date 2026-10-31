import { lstatSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";

const packageJson = JSON.parse(readFileSync("package.json", "utf8"));
const tauriConfig = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const cargoToml = readFileSync("src-tauri/Cargo.toml", "utf8");
const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const version = String(cargoVersion || "").trim();
const tag = process.env.AEROTUNE_RELEASE_TAG?.trim() || process.env.GITHUB_REF_NAME?.trim();
if (!version || packageJson.version !== version || tauriConfig.version !== version) {
  throw new Error("Cargo, package.json, and Tauri versions must be synchronized before creating latest.json.");
}
if (tag !== `v${version}`) {
  throw new Error(`Release tag must be exactly v${version}.`);
}

const bundleDirectory = resolve("src-tauri/target/release/bundle/nsis");
const entries = readdirSync(bundleDirectory);
const expectedArchiveName = `AeroTune_${version}_x64-setup.nsis.zip`;
const updaterBundles = entries.filter((name) => name === expectedArchiveName);
if (updaterBundles.length !== 1) {
  throw new Error(`Expected exactly one x64 NSIS updater archive; found ${updaterBundles.length}.`);
}
const archiveName = updaterBundles[0];
const signatureName = `${archiveName}.sig`;
const installerName = archiveName.replace(/\.nsis\.zip$/i, ".exe");
const archivePath = join(bundleDirectory, archiveName);
const signaturePath = join(bundleDirectory, signatureName);
const installerPath = join(bundleDirectory, installerName);

for (const path of [archivePath, signaturePath, installerPath]) {
  const stats = lstatSync(path);
  if (stats.isSymbolicLink() || !stats.isFile() || stats.size === 0) {
    throw new Error(`Expected a non-empty regular release asset: ${path}`);
  }
  if (path === archivePath && stats.size > 64 * 1024 * 1024) {
    throw new Error("The signed updater archive exceeds the reviewed 64 MiB transport cap.");
  }
}
const signatureText = readFileSync(signaturePath, "utf8").trim();
if (!signatureText) throw new Error("The updater .sig file is empty.");
const decodedSignature = Buffer.from(signatureText, "base64");
if (decodedSignature.toString("base64") !== signatureText) {
  throw new Error("The updater .sig file is not canonical base64.");
}
let decodedSignatureText;
try {
  decodedSignatureText = new TextDecoder("utf-8", { fatal: true }).decode(decodedSignature);
} catch {
  throw new Error("The decoded updater .sig file is not valid UTF-8.");
}
if (!decodedSignatureText.startsWith("untrusted comment:") || !decodedSignatureText.includes("trusted comment:")) {
  throw new Error("The updater .sig file does not decode to a Tauri Minisign signature.");
}
const artifactUrl = `https://github.com/shengqiangdd/AeroTune/releases/download/${tag}/${encodeURIComponent(archiveName)}`;
const manifest = {
  version,
  notes: `AeroTune ${version}`,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64-nsis": {
      signature: signatureText,
      url: artifactUrl,
    },
  },
};
const manifestPath = join(bundleDirectory, "latest.json");
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`, { flag: "w" });
console.log(JSON.stringify({
  version,
  tag,
  installer: installerPath,
  updaterArchive: archivePath,
  updaterSignature: signaturePath,
  manifest: manifestPath,
}, null, 2));
