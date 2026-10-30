import type { ReliabilityEvent, ReliabilitySnapshot } from "./api";

export type ReliabilityEventFilter = "all" | "issues" | "system" | "updates";
export interface RecurringAppGroup {
  key: string;
  label: string;
  count: number;
  crashes: number;
  hangs: number;
}
export interface ReliabilityViewState {
  eventFilter: ReliabilityEventFilter;
  appKey: string | null;
  visibleCount: number;
}
export interface ReliabilityDayBucket {
  dayKey: string;
  crashes: number;
  hangs: number;
}
export function canonicalAppKey(label: string): string;
export function buildRecurringAppGroups(events: ReliabilityEvent[]): RecurringAppGroup[];
export function filterReliabilityEvents(events: ReliabilityEvent[], eventFilter: ReliabilityEventFilter, appKey?: string | null): ReliabilityEvent[];
export function updateReliabilityFilters(current: ReliabilityViewState, changes: Partial<ReliabilityViewState>, pageSize?: number): ReliabilityViewState;
export function buildAppIssueDayBuckets(events: ReliabilityEvent[], appKey: string, rangeDays: number, rangeEndMs: number, timeZone: string): ReliabilityDayBucket[];
export function assessReliabilitySnapshot(snapshot: ReliabilitySnapshot): {
  noEvents: boolean;
  incomplete: boolean;
  truncated: boolean;
  unavailable: boolean;
  invalidRows: boolean;
};
