import type { Diagnostics } from "./api";

export type BaselineDimension = "cpu" | "mem" | "io";
export interface ProcessBaselineSnapshot {
  ready: boolean;
  cpu: number | null;
  mem: number | null;
  io: number | null;
}

export function baselineTransition(
  previous: ProcessBaselineSnapshot | null,
  diagnostics: Diagnostics | null,
  announcedDimensions?: Set<BaselineDimension>,
): { snapshot: ProcessBaselineSnapshot | null; message: string };
