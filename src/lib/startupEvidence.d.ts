import type { StartupImpact, StartupItem } from "./api";

export interface StartupEvidence {
  heuristic: string | null;
  measuredImpacts: StartupImpact[];
}

export function startupHeuristicHint(item: Partial<StartupItem> | null | undefined): string | null;
export function buildStartupEvidenceByItem(items: StartupItem[], impacts: StartupImpact[]): Map<string, StartupEvidence>;
export function startupEvidenceExportLabel(evidence: StartupEvidence | undefined): string;
