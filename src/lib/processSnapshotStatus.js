export function nextProcessSnapshotStatus(previous, succeeded) {
  if (succeeded) return { hasSnapshot: true, stale: false };
  return { hasSnapshot: previous.hasSnapshot, stale: true };
}

export function canMutateProcessRows(status, loading) {
  return status.hasSnapshot && !status.stale && !loading;
}
