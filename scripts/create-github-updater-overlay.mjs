import { readFileSync, writeFileSync, lstatSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { isValidTauriPublicKey } from "./updaterReleasePolicy.mjs";

const endpoint = "https://github.com/shengqiangdd/AeroTune/releases/latest/download/latest.json";
const publicKey = process.env.AEROTUNE_UPDATE_PUBLIC_KEY?.trim();
if (!publicKey || !isValidTauriPublicKey(publicKey)) {
  console.error("FAIL: AEROTUNE_UPDATE_PUBLIC_KEY must be a valid public Tauri Minisign key.");
  process.exit(1);
}

const templatePath = resolve("src-tauri/tauri.updater.example.json");
const overlay = JSON.parse(readFileSync(templatePath, "utf8"));
overlay.plugins.updater.pubkey = publicKey;
overlay.plugins.updater.endpoints = [endpoint];
overlay.plugins.updater.requireSignedVersion = true;
overlay.plugins.updater.allowDowngrades = false;
overlay.plugins.updater.dangerousInsecureTransportProtocol = false;
overlay.plugins.updater.dangerousAcceptInvalidCerts = false;
overlay.plugins.updater.dangerousAcceptInvalidHostnames = false;
overlay.plugins.updater.windows.installMode = "passive";
overlay.plugins.updater.windows.installerArgs = [];
overlay.bundle.createUpdaterArtifacts = true;
overlay.bundle.targets = ["nsis"];
overlay.bundle.windows.allowDowngrades = false;
overlay.bundle.windows.nsis.installMode = "perMachine";

const outputDirectory = process.env.RUNNER_TEMP?.trim() || tmpdir();
const temporaryDirectory = mkdtempSync(join(outputDirectory, "aerotune-updater-release-"));
const outputPath = join(temporaryDirectory, "tauri.updater.release.json");
writeFileSync(outputPath, `${JSON.stringify(overlay, null, 2)}\n`, { flag: "wx" });
const stats = lstatSync(outputPath);
if (stats.isSymbolicLink() || !stats.isFile()) {
  console.error("FAIL: generated updater overlay is not a regular file.");
  process.exit(1);
}
console.log(outputPath);
