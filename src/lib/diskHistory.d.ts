import type { DiskInfo } from "./api";

export interface DiskHistorySample {
  ts: number;
  mount: string;
  totalGb: number;
  freeGb: number;
}

export const DISK_HISTORY_SCHEMA: number;
export const DISK_HISTORY_SAMPLE_INTERVAL_MS: number;
export const DISK_HISTORY_RETENTION_MS: number;
export const DISK_HISTORY_MAX_SAMPLES_PER_VOLUME: number;
export const DISK_HISTORY_MAX_VOLUMES: number;
export interface DiskHistoryIndex {
  latestByMount: Map<string, DiskHistorySample>;
}
export function normalizeDriveMount(value: unknown): string | null;
export function validatedDiskFreeGb(totalGb: unknown, usedGb: unknown): number | null;
export function sanitizeDiskHistory(value: unknown, now?: number): DiskHistorySample[];
export function indexDiskHistory(history: DiskHistorySample[], now?: number): DiskHistoryIndex;
export function diskHistoryNeedsRefresh(index: DiskHistoryIndex, disks: DiskInfo[] | null | undefined, now?: number): boolean;
export function appendDiskSamples(history: DiskHistorySample[], disks: DiskInfo[] | null | undefined, now?: number): { samples: DiskHistorySample[]; changed: boolean };
