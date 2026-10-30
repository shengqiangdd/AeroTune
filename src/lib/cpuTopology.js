export const MAX_CPU_TOPOLOGY_LOGICAL_PROCESSORS = 256;

/** Normalize the displayed core values and summarize them in one bounded pass. */
export function summarizeCpuTopology(values) {
  const source = values ?? [];
  const count = Math.min(source.length, MAX_CPU_TOPOLOGY_LOGICAL_PROCESSORS);
  const logical = [];
  let sum = 0;
  let peak = -Infinity;

  for (let index = 0; index < count; index += 1) {
    const sample = source[index];
    const value = Number.isFinite(sample) ? Math.max(0, Math.min(100, sample)) : 0;
    logical.push(value);
    sum += value;
    if (value > peak) peak = value;
  }

  return {
    logical,
    mean: count > 0 ? sum / count : null,
    peak: count > 0 ? peak : null,
  };
}
