export const MAX_CPU_TOPOLOGY_LOGICAL_PROCESSORS: 256;
export function summarizeCpuTopology(values: readonly number[] | null | undefined): {
  logical: number[];
  mean: number | null;
  peak: number | null;
};
