/** Serialize diagnostic polling with manual refreshes while preserving await semantics. */
export function createRefreshCoordinator({ runRefresh, runPoll, setRefreshing, schedulePoll }) {
  let pending = false;
  let refreshQueued = false;
  let disposed = false;
  const waiters = [];

  const resolveWaiters = () => {
    for (const resolve of waiters.splice(0)) resolve();
  };

  const refresh = async () => {
    if (disposed) return;
    if (pending) {
      refreshQueued = true;
      setRefreshing(true);
      return new Promise((resolve) => { waiters.push(resolve); });
    }

    pending = true;
    setRefreshing(true);
    try {
      await runRefresh();
    } finally {
      pending = false;
      if (!disposed && refreshQueued) {
        refreshQueued = false;
        void refresh();
      } else {
        refreshQueued = false;
        if (!disposed) {
          setRefreshing(false);
          schedulePoll();
        }
        resolveWaiters();
      }
    }
  };

  const poll = async () => {
    if (disposed) return;
    if (pending) {
      schedulePoll();
      return;
    }

    pending = true;
    try {
      await runPoll();
    } finally {
      pending = false;
      if (!disposed && refreshQueued) {
        refreshQueued = false;
        void refresh();
      } else if (!disposed) {
        schedulePoll();
      } else {
        refreshQueued = false;
        resolveWaiters();
      }
    }
  };

  const dispose = () => {
    disposed = true;
    refreshQueued = false;
    resolveWaiters();
  };

  return { refresh, poll, dispose };
}
