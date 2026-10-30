export function startupDelayRecoveryViewState(status, records) {
  const hasSnapshot = records !== null;
  const hasRecords = Array.isArray(records) && records.length > 0;
  return {
    loading: status === "loading",
    refreshingSnapshot: status === "loading" && hasSnapshot,
    error: status === "error",
    showEmpty: status === "ready" && hasSnapshot && !hasRecords,
    showRecords: hasRecords,
    canRestore: status === "ready" && hasSnapshot,
    hasSnapshot,
  };
}
