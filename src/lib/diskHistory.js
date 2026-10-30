export const DISK_HISTORY_SCHEMA = 1;
export const DISK_HISTORY_SAMPLE_INTERVAL_MS = 30 * 60 * 1000;
export const DISK_HISTORY_RETENTION_MS = 14 * 24 * 60 * 60 * 1000;
export const DISK_HISTORY_MAX_SAMPLES_PER_VOLUME = 673; // Includes both endpoints of the 14-day window.
export const DISK_HISTORY_MAX_VOLUMES = 16;

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function normalizeDriveMount(value) {
  if (typeof value !== "string") return null;
  const match = /^([a-z]):(?:[\\/])?$/i.exec(value.trim());
  return match ? `${match[1].toUpperCase()}:` : null;
}

export function validatedDiskFreeGb(totalGb, usedGb) {
  if (typeof totalGb !== "number" || !Number.isFinite(totalGb) || totalGb <= 0) return null;
  if (typeof usedGb !== "number" || !Number.isFinite(usedGb) || usedGb < 0 || usedGb > totalGb) return null;
  return totalGb - usedGb;
}

function validSample(value, now) {
  if (!isRecord(value)) return null;
  const mount = normalizeDriveMount(value.mount);
  const ts = value.ts;
  const totalGb = value.totalGb;
  const freeGb = value.freeGb;
  if (!mount || !Number.isSafeInteger(ts) || ts < now - DISK_HISTORY_RETENTION_MS || ts > now) return null;
  if (typeof totalGb !== "number" || !Number.isFinite(totalGb) || totalGb <= 0) return null;
  if (typeof freeGb !== "number" || !Number.isFinite(freeGb) || freeGb < 0 || freeGb > totalGb) return null;
  return { ts, mount, totalGb, freeGb };
}

export function sanitizeDiskHistory(value, now = Date.now()) {
  const samples = Array.isArray(value) ? value : isRecord(value) && value.version === DISK_HISTORY_SCHEMA && Array.isArray(value.samples) ? value.samples : [];
  const byMount = new Map();
  for (const entry of samples) {
    const sample = validSample(entry, now);
    if (!sample) continue;
    const list = byMount.get(sample.mount) ?? [];
    list.push(sample);
    byMount.set(sample.mount, list);
  }
  for (const list of byMount.values()) list.sort((a, b) => a.ts - b.ts);
  return [...byMount.entries()]
    .sort(([mountA, a], [mountB, b]) => (b[b.length - 1]?.ts ?? 0) - (a[a.length - 1]?.ts ?? 0) || mountA.localeCompare(mountB))
    .slice(0, DISK_HISTORY_MAX_VOLUMES)
    .flatMap(([, list]) => list.slice(-DISK_HISTORY_MAX_SAMPLES_PER_VOLUME));
}

export function indexDiskHistory(historyValue, now = Date.now()) {
  const history = sanitizeDiskHistory(historyValue, now);
  const latestByMount = new Map();
  for (const sample of history) {
    const latest = latestByMount.get(sample.mount);
    if (!latest || sample.ts > latest.ts) latestByMount.set(sample.mount, sample);
  }
  return { latestByMount };
}

export function diskHistoryNeedsRefresh(index, disks, now = Date.now()) {
  return (Array.isArray(disks) ? disks : []).some((disk) => {
    if (!isRecord(disk)) return false;
    const mount = normalizeDriveMount(disk.mount);
    const totalGb = disk.total_gb;
    const usedGb = disk.used_gb;
    if (!mount || validatedDiskFreeGb(totalGb, usedGb) === null) return false;
    const last = index?.latestByMount?.get(mount);
    if (!last) return true;
    if (Math.abs(totalGb - last.totalGb) / last.totalGb > 0.2) return true;
    return now - last.ts >= DISK_HISTORY_SAMPLE_INTERVAL_MS;
  });
}

export function appendDiskSamples(historyValue, disks, now = Date.now()) {
  const history = sanitizeDiskHistory(historyValue, now);
  const latestByMount = new Map();
  for (const sample of history) {
    const latest = latestByMount.get(sample.mount);
    if (!latest || sample.ts > latest.ts) latestByMount.set(sample.mount, sample);
  }

  const acceptedDisks = (Array.isArray(disks) ? disks : [])
    .slice(0, DISK_HISTORY_MAX_VOLUMES)
    .map((disk) => {
      if (!isRecord(disk)) return null;
      const mount = normalizeDriveMount(disk.mount);
      const totalGb = disk.total_gb;
      const freeGb = validatedDiskFreeGb(totalGb, disk.used_gb);
      if (!mount || freeGb === null) return null;
      return { mount, totalGb, freeGb };
    })
    .filter(Boolean);

  let next = history;
  let changed = false;
  for (const disk of acceptedDisks) {
    let last = latestByMount.get(disk.mount);
    if (last && Math.abs(disk.totalGb - last.totalGb) / last.totalGb > 0.2) {
      next = next.filter((sample) => sample.mount !== disk.mount);
      latestByMount.delete(disk.mount);
      last = undefined;
      changed = true;
    }
    if (last && now - last.ts < DISK_HISTORY_SAMPLE_INTERVAL_MS) continue;
    next = [...next, { ts: Math.floor(now), mount: disk.mount, totalGb: disk.totalGb, freeGb: disk.freeGb }];
    latestByMount.set(disk.mount, next[next.length - 1]);
    changed = true;
  }

  return { samples: sanitizeDiskHistory(next, now), changed };
}
