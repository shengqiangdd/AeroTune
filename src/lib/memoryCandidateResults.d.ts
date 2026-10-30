export function mergeMemoryCandidateResults<
  T extends { pid: number; eligible: boolean; reason: string; creation_token?: string },
>(
  previewCandidates: T[],
  attemptedPids: number[],
  returnedCandidates: Array<{ pid: number; reason: string }>,
): T[];
