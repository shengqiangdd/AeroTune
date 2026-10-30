import type { MemoryOptimizationReport } from "./api";

export function aggregateMemoryOptimizationBatches(
  reports: MemoryOptimizationReport[],
  fallback: MemoryOptimizationReport | null,
  interrupted?: boolean,
): MemoryOptimizationReport | null;
