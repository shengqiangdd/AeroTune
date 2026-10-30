import type { Diagnostics, Stats } from "./api";

export function validCapacityPair(used: unknown, total: unknown): boolean;

export interface ResourceEvidence {
  label: string;
  value: string;
  measured: boolean;
  pending?: boolean;
}

export function gpuEvidence(stats: Stats | null): ResourceEvidence[];
export function cpuEvidence(diagnostics: Diagnostics | null): ResourceEvidence[];
export function ioEvidence(diagnostics: Diagnostics | null): ResourceEvidence[];
export function diskEvidence(stats: Stats | null): ResourceEvidence[];
