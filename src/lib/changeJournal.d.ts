import type { Change } from "./api";

export type ChangeCategoryFilter = "all" | "registry" | "power_plan" | "tcp" | "dns" | "other";
export type ChangeReversibilityFilter = "all" | "reversible" | "info";

export function getChangeCategory(type: string): string;
export function getChangeRevertTarget(change: Change): string;
export function summarizeUndoTargets(changes: Change[], limit?: number): {
  complete: boolean;
  tooMany: boolean;
  tooLong: boolean;
  invalidTarget: boolean;
  total: number;
  visible: string[];
};
export function filterChanges(
  changes: Change[],
  query?: string,
  category?: ChangeCategoryFilter,
  reversibility?: ChangeReversibilityFilter,
): Change[];
export function buildChangeJournalCsv(changes: Change[], appVersion: string): string;
export function buildChangeJournalJson(changes: Change[], appVersion: string): string;
