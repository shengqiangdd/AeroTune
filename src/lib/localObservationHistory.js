export const LOCAL_OBSERVATION_HISTORY_TARGETS = Object.freeze([
  Object.freeze({ id: "network", label: "网络质量历史样本", storage: "localStorage", key: "aerotune.network-quality-history.v1" }),
  Object.freeze({ id: "disk", label: "磁盘空间趋势样本", storage: "localStorage", key: "aerotune.disk-space-history.v1" }),
  Object.freeze({ id: "process", label: "进程资源历史及采样开关", storage: "localStorage", key: "aerotune.process-history.v1" }),
  Object.freeze({ id: "battery", label: "电池续航样本", storage: "localStorage", key: "aerotune.power-insights.samples.v1" }),
  Object.freeze({ id: "experiments", label: "优化实验历史", storage: "localStorage", key: "aerotune.optimization-lab.history.v1" }),
]);

function getStorage(storageName, overrides) {
  try {
    if (Object.prototype.hasOwnProperty.call(overrides, storageName)) return overrides[storageName];
    return globalThis[storageName] ?? null;
  } catch {
    return null;
  }
}

/** Remove only the enumerated AeroTune WebView history keys; never clear the whole storage origin. */
export function clearLocalObservationHistory(ids = LOCAL_OBSERVATION_HISTORY_TARGETS.map(({ id }) => id), storageOverrides = {}) {
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
