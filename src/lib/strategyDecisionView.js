export function strategyDecisionViewState(backendReady, adaptive, scoreCount) {
  if (!backendReady || (adaptive !== true && adaptive !== false)) return "waiting";
  const hasSnapshot = Number.isInteger(scoreCount) && scoreCount > 0;
  if (adaptive) return hasSnapshot ? "active" : "warming";
  return hasSnapshot ? "paused_snapshot" : "paused_empty";
}
