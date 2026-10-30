export type CpuLoadBand = "unknown" | "normal" | "elevated" | "high";
export type MemoryLoadBand = "unknown" | "normal" | "elevated" | "high";
export type InteractionLatencyBand = "unknown" | "normal" | "elevated" | "high";

export function cpuLoadBandFromUsage(
  cpuPct: number | undefined,
  previousBand?: CpuLoadBand,
): CpuLoadBand;

export function memoryLoadBandFromUsage(
  usedGb: number | undefined,
  totalGb: number | undefined,
  previousBand?: MemoryLoadBand,
): MemoryLoadBand;

export function interactionLatencyBandFromP95(
  p95Ms: number | null,
  sampleCount: number,
  previousBand?: InteractionLatencyBand,
): InteractionLatencyBand;

export function processRefreshIntervalMs(
  cpuLoadBand: CpuLoadBand,
  memoryLoadBand: MemoryLoadBand,
  interactionLatencyBand?: InteractionLatencyBand,
): number;
