import type { SmartRecommendation } from "./api";

export type SymptomFocusId = "lag" | "gpu" | "memory" | "io" | "startup" | "battery" | "disk";
export interface SymptomFocus {
  symptom: SymptomFocusId;
  recommendation: SmartRecommendation;
}

export function symptomForRecommendation(id: string): SymptomFocusId | null;
export function rankSymptomFocus(recommendations: SmartRecommendation[]): SymptomFocus | null;
