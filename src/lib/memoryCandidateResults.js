/**
 * Merge an attempted memory action into its preview without masking backend reasons.
 * Unattempted candidates stay selectable; attempted rows must be re-analyzed.
 *
 * @param {Array<{pid: number, eligible: boolean, reason: string, creation_token?: string}>} previewCandidates
 * @param {number[]} attemptedPids
 * @param {Array<{pid: number, reason: string}>} returnedCandidates
 */
export function mergeMemoryCandidateResults(previewCandidates, attemptedPids, returnedCandidates) {
  const attempted = new Set(attemptedPids);
  const resultsByPid = new Map(returnedCandidates.map((candidate) => [candidate.pid, candidate]));

  return previewCandidates.map((candidate) => {
    if (!attempted.has(candidate.pid)) return candidate;
    const result = resultsByPid.get(candidate.pid);
    return {
      ...candidate,
      eligible: false,
      reason: result?.reason ?? "本次操作结果未返回，请重新分析",
      creation_token: undefined,
    };
  });
}
