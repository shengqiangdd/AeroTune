export type StartupDelayRecoveryStatus = "loading" | "ready" | "error";

export type StartupDelayRecoveryView = {
  loading: boolean;
  refreshingSnapshot: boolean;
  error: boolean;
  showEmpty: boolean;
  showRecords: boolean;
  canRestore: boolean;
  hasSnapshot: boolean;
};

export function startupDelayRecoveryViewState(
  status: StartupDelayRecoveryStatus,
  records: readonly { name: string; task: string }[] | null,
): StartupDelayRecoveryView;
