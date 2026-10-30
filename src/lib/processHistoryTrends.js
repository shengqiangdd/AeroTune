export function processNameKey(name) {
  return name.toLowerCase();
}

export function clearLegacyProcessHistoryCpu(samples) {
  return samples.map((sample) => ({
    sampledAt: sample.sampledAt,
    processes: sample.processes.map((metric) => ({
      name: metric.name,
      cpu: null,
      memMb: metric.memMb,
    })),
  }));
}

export function mergeProcessHistoryMetrics(metrics) {
  const byName = new Map();
  for (const metric of metrics) {
    const key = processNameKey(metric.name);
    const current = byName.get(key);
    if (current) {
      if (metric.cpu != null) current.cpu = Math.min(100, (current.cpu ?? 0) + metric.cpu);
      current.memMb = Math.min(10_000_000, current.memMb + metric.memMb);
    } else {
      byName.set(key, { ...metric, cpu: metric.cpu ?? null });
    }
  }
  return [...byName.values()].map((metric) => ({
    name: metric.name,
    cpu: metric.cpu == null ? null : Math.round(Math.min(100, metric.cpu) * 10) / 10,
    memMb: Math.round(metric.memMb * 10) / 10,
  }));
}

export function buildTrends(samples) {
  const byName = new Map();
  // Callers pass the validated, chronologically pruned history.
  for (const sample of samples) {
    for (const metric of sample.processes) {
      const nameKey = processNameKey(metric.name);
      let values = byName.get(nameKey);
      if (!values) {
        values = {
          name: metric.name,
          cpu: [],
          memMb: [],
          firstCpuSampledAt: null,
          lastCpuSampledAt: null,
          firstMemorySampledAt: sample.sampledAt,
          lastMemorySampledAt: sample.sampledAt,
        };
        byName.set(nameKey, values);
      }
      if (metric.cpu != null) {
        if (values.cpu.length === 0) values.firstCpuSampledAt = sample.sampledAt;
        values.cpu.push(metric.cpu);
        values.lastCpuSampledAt = sample.sampledAt;
      }
      values.memMb.push(metric.memMb);
      values.lastMemorySampledAt = sample.sampledAt;
    }
  }

  return [...byName.values()].map((values) => ({
    name: values.name,
    cpuObservations: values.cpu.length,
    memoryObservations: values.memMb.length,
    averageCpu: values.cpu.length
      ? values.cpu.reduce((sum, value) => sum + value, 0) / values.cpu.length
      : null,
    averageMemMb: values.memMb.reduce((sum, value) => sum + value, 0) / values.memMb.length,
    firstCpu: values.cpu.length ? values.cpu[0] : null,
    lastCpu: values.cpu.length ? values.cpu[values.cpu.length - 1] : null,
    firstMemMb: values.memMb[0],
    lastMemMb: values.memMb[values.memMb.length - 1],
    firstCpuSampledAt: values.firstCpuSampledAt,
    lastCpuSampledAt: values.lastCpuSampledAt,
    firstMemorySampledAt: values.firstMemorySampledAt,
    lastMemorySampledAt: values.lastMemorySampledAt,
  }));
}

export function formatProcessHistorySpan(spanMs) {
  const minutes = Math.round(spanMs / 60_000);
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = minutes / 60;
  return hours < 24 ? `${hours.toFixed(1)} 小时` : `${(hours / 24).toFixed(1)} 天`;
}

