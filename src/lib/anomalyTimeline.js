// Positions and labels for an anomaly-centred timeline. Events may occur after onset.
export function formatAnomalyOffset(time, onset) {
  const delta = Math.round(time - onset);
  return delta === 0 ? "起点" : `${delta < 0 ? "−" : "+"}${Math.abs(delta)}秒`;
}

export function anomalyTimelineLayout(events, onset) {
  const times = events.map((event) => event.at_unix).filter(Number.isFinite);
  const min = Math.min(onset, ...times) - 15;
  const max = Math.max(onset + 15, ...times.map((time) => time + 15), min + 60);
  const span = max - min;
  const step = Math.max(30, Math.ceil(span / 300) * 60);
  const ticks = [];
  for (let time = min + step; time < max && ticks.length < 6; time += step) {
    ticks.push({ time, label: formatAnomalyOffset(time, onset) });
  }
  return { min, max, span, ticks };
}
