export function formatAnomalyOffset(time: number, onset: number): string;
export function anomalyTimelineLayout(events: ReadonlyArray<{ at_unix: number }>, onset: number): {
  min: number;
  max: number;
  span: number;
  ticks: Array<{ time: number; label: string }>;
};
