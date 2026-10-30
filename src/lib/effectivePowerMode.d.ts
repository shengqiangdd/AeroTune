import type { EffectivePowerMode } from "./api";

export type EffectivePowerModeTone = "slate" | "cyan" | "violet" | "green" | "amber" | "rose";
export interface EffectivePowerModePresentation {
  label: string;
  tone: EffectivePowerModeTone;
  description: string;
}

export function effectivePowerModePresentation(
  mode: EffectivePowerMode | null | undefined,
  loading?: boolean,
  stale?: boolean,
): EffectivePowerModePresentation;
export function effectivePowerModeContext(
  mode: EffectivePowerMode | null | undefined,
  aeroTunePowerEnabled: boolean | null | undefined,
): string | null;
