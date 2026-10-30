export type StrategyDecisionViewState = "waiting" | "warming" | "active" | "paused_snapshot" | "paused_empty";

export function strategyDecisionViewState(
  backendReady: boolean,
  adaptive: boolean | null,
  scoreCount: number,
): StrategyDecisionViewState;
