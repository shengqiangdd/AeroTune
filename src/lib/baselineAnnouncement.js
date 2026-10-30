const DIMENSIONS = [
  ["cpu", "cpu_baseline_warming_processes", "CPU"],
  ["mem", "mem_baseline_warming_processes", "内存"],
  ["io", "io_baseline_warming_processes", "I/O"],
];

function warmingCount(diagnostics, field) {
  const value = diagnostics[field];
  return Number.isSafeInteger(value) && value >= 0 ? value : null;
}

export function baselineTransition(previous, diagnostics, announcedDimensions = new Set()) {
  if (!diagnostics) return { snapshot: null, message: "" };

  const snapshot = {
    ready: diagnostics.process_sample_ready === true,
    cpu: warmingCount(diagnostics, "cpu_baseline_warming_processes"),
    mem: warmingCount(diagnostics, "mem_baseline_warming_processes"),
    io: warmingCount(diagnostics, "io_baseline_warming_processes"),
  };
  if (!previous) return { snapshot, message: "" };

  if (previous.ready && !snapshot.ready) {
    announcedDimensions.clear();
    return { snapshot, message: "进程采样已重置，诊断基线将在新样本到达后重新建立。" };
  }
  if (!snapshot.ready) return { snapshot, message: "" };

  if (!previous.ready) {
    const counts = [snapshot.cpu, snapshot.mem, snapshot.io];
    const message = counts.some((count) => count === null)
      ? "进程样本已就绪，基线覆盖状态正在确认。"
      : counts.some((count) => count > 0)
        ? "进程样本已就绪，部分进程基线仍在建立。"
        : "进程样本已就绪，当前无需等待进程基线。";
    return { snapshot, message };
  }

  const completed = [];
  for (const [dimension, , label] of DIMENSIONS) {
    if (snapshot[dimension] === 0
      && previous[dimension] !== 0
      && !announcedDimensions.has(dimension)) {
      announcedDimensions.add(dimension);
      completed.push(label);
    }
  }
  return {
    snapshot,
    message: completed.length ? `基线已就绪：${completed.join("、")}。` : "",
  };
}
