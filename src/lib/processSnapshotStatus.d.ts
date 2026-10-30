export type ProcessSnapshotStatus = { hasSnapshot: boolean; stale: boolean };
export function nextProcessSnapshotStatus(previous: ProcessSnapshotStatus, succeeded: boolean): ProcessSnapshotStatus;
export function canMutateProcessRows(status: ProcessSnapshotStatus, loading: boolean): boolean;
