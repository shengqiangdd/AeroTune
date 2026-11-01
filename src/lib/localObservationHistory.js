const MAX_LOCAL_STORAGE_KEYS_TO_INSPECT = 512;

export const LOCAL_OBSERVATION_HISTORY_TARGETS = Object.freeze([
  Object.freeze({
    id: "network",
    label: "网络质量历史样本",
    detail: "时间、探测数、延迟/抖动/丢包；用户主动观察后产生，最多 300 条且不超过 24 小时。",
    storage: "localStorage",
    key: "aerotune.network-quality-history.v1",
  }),
  Object.freeze({
    id: "disk",
    label: "磁盘空间趋势样本",
    detail: "盘符、容量、可用空间和采样时间；仪表盘采样，保留 14 天。",
    storage: "localStorage",
    key: "aerotune.disk-space-history.v1",
  }),
  Object.freeze({
    id: "process",
    label: "进程资源历史及采样开关",
    detail: "程序名及 CPU/内存汇总；默认关闭，开启后每 5 分钟最多采样一次、每次最多 20 个名称，保留 7 天。清除此项会恢复默认关闭。",
    storage: "localStorage",
    key: "aerotune.process-history.v1",
  }),
  Object.freeze({
    id: "battery",
    label: "电池续航样本",
    detail: "采样时间、电量和供电状态；默认关闭，开启后保留 48 小时。",
    storage: "localStorage",
    key: "aerotune.power-insights.samples.v1",
  }),
  Object.freeze({
    id: "experiments",
    label: "优化实验历史",
    detail: "最多 8 条实验汇总；可恢复的实验会话草稿单独保留，不在此清理。",
    storage: "localStorage",
    key: "aerotune.optimization-lab.history.v1",
  }),
]);

function getStorage(storageName, overrides) {
  try {
    if (Object.prototype.hasOwnProperty.call(overrides, storageName)) return overrides[storageName];
    return globalThis[storageName] ?? null;
  } catch {
    return null;
  }
}

/** Inspect allowlisted key names only; never read stored history values. */
export function inspectLocalObservationHistory(storageOverrides = {}) {
  let presentKeys;
  try {
    const storage = getStorage("localStorage", storageOverrides);
    if (!storage || typeof storage.key !== "function") throw new Error("storage unavailable");
    const length = storage.length;
    if (!Number.isSafeInteger(length) || length < 0 || length > MAX_LOCAL_STORAGE_KEYS_TO_INSPECT) {
      throw new Error("storage inspection bound exceeded");
    }
    const allowedKeys = new Set(LOCAL_OBSERVATION_HISTORY_TARGETS.map(({ key }) => key));
    presentKeys = new Set();
    for (let index = 0; index < length; index += 1) {
      const key = storage.key(index);
      if (allowedKeys.has(key)) presentKeys.add(key);
    }
  } catch {
    return LOCAL_OBSERVATION_HISTORY_TARGETS.map(({ id, label }) => ({ id, label, state: "unknown" }));
  }

  return LOCAL_OBSERVATION_HISTORY_TARGETS.map((target) => ({
    id: target.id,
    label: target.label,
    state: presentKeys.has(target.key) ? "present" : "absent",
  }));
}

/** Remove only the enumerated AeroTune WebView history keys; never clear the whole storage origin. */
export function clearLocalObservationHistory(ids = [], storageOverrides = {}) {
  const requested = new Set(Array.isArray(ids) ? ids : []);
  const results = [];
  for (const target of LOCAL_OBSERVATION_HISTORY_TARGETS) {
    if (!requested.has(target.id)) continue;
    try {
      const storage = getStorage(target.storage, storageOverrides);
      if (!storage) throw new Error("storage unavailable");
      storage.removeItem(target.key);
      results.push({ id: target.id, label: target.label, ok: true });
    } catch {
      results.push({ id: target.id, label: target.label, ok: false });
    }
  }
  return results;
}
