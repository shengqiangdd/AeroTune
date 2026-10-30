import type { StartupItem } from "./api";

export type StartupImpactPathMatch =
  | { kind: "exact"; item: StartupItem }
  | { kind: "ambiguous"; items: StartupItem[] }
  | { kind: "none" };

export function matchStartupImpactPathToItem(
  impactPath: string | null | undefined,
  items: readonly StartupItem[],
): StartupImpactPathMatch;
