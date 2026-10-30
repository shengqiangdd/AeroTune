export interface RefreshCoordinatorOptions {
  runRefresh: () => Promise<void>;
  runPoll: () => Promise<void>;
  setRefreshing: (value: boolean) => void;
  schedulePoll: () => void;
}

export interface RefreshCoordinator {
  refresh: () => Promise<void>;
  poll: () => Promise<void>;
  dispose: () => void;
}

export function createRefreshCoordinator(
  options: RefreshCoordinatorOptions,
): RefreshCoordinator;
