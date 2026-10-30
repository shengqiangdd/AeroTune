import { lstatSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { tmpdir } from "node:os";
import { readUpdaterReleaseConfigFile } from "./updaterReleasePolicy.mjs";

const SNAPSHOT_DIRECTORY_PREFIX = "aerotune-updater-overlay-";
const SNAPSHOT_FILE_NAME = "release-overlay.json";

export function createValidatedUpdaterOverlaySnapshot(sourcePath) {
  const config = readUpdaterReleaseConfigFile(sourcePath);
  const directory = mkdtempSync(join(tmpdir(), SNAPSHOT_DIRECTORY_PREFIX));
  const snapshotPath = join(directory, SNAPSHOT_FILE_NAME);
  try {
    writeFileSync(snapshotPath, JSON.stringify(config), { flag: "wx", mode: 0o600 });
    return Object.freeze({ directory, path: snapshotPath, config });
  } catch (error) {
    removeValidatedUpdaterOverlaySnapshot({ directory, path: snapshotPath });
    throw error;
  }
}

export function removeValidatedUpdaterOverlaySnapshot(snapshot) {
  const tempRoot = resolve(tmpdir());
  const directory = resolve(snapshot?.directory ?? "");
  const path = resolve(snapshot?.path ?? "");
  if (
    dirname(directory) !== tempRoot
    || !basename(directory).startsWith(SNAPSHOT_DIRECTORY_PREFIX)
    || dirname(path) !== directory
    || basename(path) !== SNAPSHOT_FILE_NAME
  ) {
    throw new Error("Refusing to remove an unexpected updater overlay snapshot path.");
  }
  const directoryStats = lstatSync(directory);
  if (directoryStats.isSymbolicLink() || !directoryStats.isDirectory()) {
    throw new Error("Refusing to remove a non-directory updater overlay snapshot path.");
  }
  try {
    const snapshotStats = lstatSync(path);
    if (snapshotStats.isSymbolicLink() || !snapshotStats.isFile()) {
      throw new Error("Refusing to remove a non-file updater overlay snapshot.");
    }
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  rmSync(directory, { recursive: true, force: true });
}
