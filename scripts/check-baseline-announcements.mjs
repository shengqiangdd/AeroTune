import assert from "node:assert/strict";
import { baselineTransition } from "../src/lib/baselineAnnouncement.js";

const announced = new Set();
let previous = null;
const next = (ready, cpu, mem, io) => {
  const result = baselineTransition(previous, {
    process_sample_ready: ready,
    cpu_baseline_warming_processes: cpu,
    mem_baseline_warming_processes: mem,
    io_baseline_warming_processes: io,
  }, announced);
  previous = result.snapshot;
  return result.message;
};

assert.equal(next(false, 0, 0, 0), "");
assert.equal(next(true, 4, 2, 1), "进程样本已就绪，部分进程基线仍在建立。");
assert.equal(next(true, 3, 2, 1), "", "countdown changes must not chatter");
assert.equal(next(true, 0, 2, 1), "基线已就绪：CPU。");
assert.equal(next(true, 0, 0, 0), "基线已就绪：内存、I/O。");
assert.equal(next(true, 1, 0, 0), "", "a newly warming process must not repeat a completed announcement");
assert.equal(next(true, 0, 0, 0), "", "the completed dimension is announced once per sampling cycle");
assert.equal(next(false, 0, 0, 0), "进程采样已重置，诊断基线将在新样本到达后重新建立。");
assert.equal(announced.size, 0, "sampling reset allows future readiness milestones");
assert.equal(next(true, 1, 1, 1), "进程样本已就绪，部分进程基线仍在建立。");
assert.equal(next(true, 0, 0, 0), "基线已就绪：CPU、内存、I/O。");

const unknown = baselineTransition(null, {
  process_sample_ready: true,
  cpu_baseline_warming_processes: -1,
  mem_baseline_warming_processes: Number.NaN,
  io_baseline_warming_processes: 2,
}, new Set());
assert.equal(unknown.message, "");
assert.equal(unknown.snapshot.cpu, null);
assert.equal(unknown.snapshot.mem, null);
assert.equal(unknown.snapshot.io, 2);

console.log("Baseline announcements passed: sample start, quiet countdowns, per-dimension readiness, and reset");
