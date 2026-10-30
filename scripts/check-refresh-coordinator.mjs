import assert from "node:assert/strict";
import { createRefreshCoordinator } from "../src/lib/refreshCoordinator.js";

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

const pollGate = deferred();
const refreshGate = deferred();
const refreshStarted = deferred();
const events = [];
const busyStates = [];
let scheduledPolls = 0;
let pollCalls = 0;
let refreshCalls = 0;
const coordinator = createRefreshCoordinator({
  runRefresh: async () => {
    refreshCalls += 1;
    events.push("refresh:start");
    refreshStarted.resolve();
    await refreshGate.promise;
    events.push("refresh:end");
  },
  runPoll: async () => {
    pollCalls += 1;
    events.push("poll:start");
    await pollGate.promise;
    events.push("poll:end");
  },
  setRefreshing: (value) => busyStates.push(value),
  schedulePoll: () => { scheduledPolls += 1; },
});

const poll = coordinator.poll();
assert.equal(pollCalls, 1);
assert.deepEqual(busyStates, [], "background polling does not show manual busy state");
let firstSettled = false;
let secondSettled = false;
const firstManual = coordinator.refresh().then(() => { firstSettled = true; });
const secondManual = coordinator.refresh().then(() => { secondSettled = true; });
await Promise.resolve();
assert.deepEqual(busyStates, [true, true], "queued clicks show busy immediately");
assert.equal(firstSettled, false, "manual refresh waits for the active poll and follow-up fetch");
assert.equal(secondSettled, false, "coalesced caller waits for the same follow-up fetch");

pollGate.resolve();
await poll;
await refreshStarted.promise;
assert.equal(refreshCalls, 1, "concurrent manual requests coalesce to one follow-up fetch");
assert.equal(firstSettled, false);
assert.equal(secondSettled, false);
refreshGate.resolve();
await Promise.all([firstManual, secondManual]);

assert.deepEqual(events, ["poll:start", "poll:end", "refresh:start", "refresh:end"]);
assert.equal(scheduledPolls, 1, "polling resumes once after the queued fetch settles");
assert.deepEqual(busyStates, [true, true, true, false]);

const disposePollGate = deferred();
let postDisposeRefreshes = 0;
const disposedCoordinator = createRefreshCoordinator({
  runRefresh: async () => { postDisposeRefreshes += 1; },
  runPoll: () => disposePollGate.promise,
  setRefreshing: () => {},
  schedulePoll: () => {},
});
const disposingPoll = disposedCoordinator.poll();
const abandonedCaller = disposedCoordinator.refresh();
let abandonedSettled = false;
void abandonedCaller.then(() => { abandonedSettled = true; });
disposedCoordinator.dispose();
await abandonedCaller;
assert.equal(abandonedSettled, true, "dispose settles queued callers instead of leaving them pending");
disposePollGate.resolve();
await disposingPoll;
assert.equal(postDisposeRefreshes, 0, "dispose prevents queued work from restarting after teardown");

console.log("Refresh coordinator passed: queued await, coalescing, busy feedback, poll rescheduling, and teardown");
