/**
 * Combine confirmed memory-optimization batch reports while keeping measurements honest.
 * The first pre-action reading remains meaningful even when it is exactly 0 MB; if any
 * batch outcome is unknown, a post-action reading is no longer presented as final.
 *
 * @param {Array<{
 *   available_before_mb: number,
 *   available_after_mb: number | null,
 *   processed: number,
 *   touched_working_set_mb: number,
 *   estimated_reclaimed_mb: number,
 *   skipped: number,
 * }>} reports
 * @param {object | null} fallback
 * @param {boolean} interrupted
 */
export function aggregateMemoryOptimizationBatches(reports, fallback, interrupted = false) {
  const finalReport = reports.at(-1) ?? fallback;
  if (!finalReport) return null;
  const firstReport = reports[0] ?? fallback;
  const totals = reports.reduce((sum, report) => ({
    processed: sum.processed + report.processed,
    touched_working_set_mb: sum.touched_working_set_mb + report.touched_working_set_mb,
    estimated_reclaimed_mb: sum.estimated_reclaimed_mb + report.estimated_reclaimed_mb,
    skipped: sum.skipped + report.skipped,
  }), {
    processed: 0,
    touched_working_set_mb: 0,
    estimated_reclaimed_mb: 0,
    skipped: 0,
  });

  return {
    ...finalReport,
    available_before_mb: firstReport?.available_before_mb ?? finalReport.available_before_mb,
    available_after_mb: interrupted ? null : finalReport.available_after_mb,
    ...totals,
  };
}
