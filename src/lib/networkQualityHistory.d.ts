import type { NetQuality, PingStat } from "./api";

export type NetworkQualityHistoryEntry = Pick<NetQuality, "gateway" | "dns" | "second_dns"> & {
  ts: number;
};

export const NETWORK_QUALITY_HISTORY_MAX_AGE_MS: number;
export const NETWORK_QUALITY_HISTORY_MAX_ENTRIES: number;
export const NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS: number;
export function parseNetworkQualityHistoryEntry(
  value: unknown,
  now: number,
): NetworkQualityHistoryEntry | null;
export function trimNetworkQualityHistory(
  entries: NetworkQualityHistoryEntry[],
  now: number,
): NetworkQualityHistoryEntry[];
export function normalizeNetworkQualityEntry(
  quality: NetQuality,
  ts: number,
): NetworkQualityHistoryEntry | null;
