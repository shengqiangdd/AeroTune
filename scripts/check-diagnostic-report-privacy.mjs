import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parseDiagnosticReport, redactDiagnosticReport } from "../src/lib/diagnosticReport.js";

const dashboard = readFileSync(new URL("../src/pages/Dashboard.tsx", import.meta.url), "utf8");

const original = {
  schema_version: 1,
  app_version: "1.5.9",
  future_top_level_secret: "FUTURE-TOP-SECRET",
  generated_at_utc: "2026-10-20T09:00:00Z",
  generated_at_local: "2026-10-20T17:00:00+08:00",
  system: { os: "Windows", version: "11.0", arch: "x64", host: "LAPTOP-SECRET", cpu_model: "CPU SECRET", cores: 8, admin: false, future_device_secret: "FUTURE-SYSTEM-SECRET" },
  runtime: {
    cpu_pct: 18,
    memory_used_gb: 8,
    memory_total_gb: 16,
    pagefile_used_gb: 1,
    pagefile_total_gb: 8,
    process_count: 150,
    uptime_secs: 86400,
    sample_id: 42,
    health_score: 91,
    cpu_frequency_mhz: 3200,
    memory_pressure_pct: 50,
    pagefile_pressure_pct: 12.5,
    swap_in_use: true,
    has_sample: true,
    network_rx_kbps: 8,
    network_tx_kbps: 2,
    network_today_rx_gb: 5,
    network_today_tx_gb: 1,
    battery_percent: 70,
    on_ac_power: false,
    gpu_name: "GPU SECRET",
    gpu_utilization_pct: 20,
    gpu_temperature_c: 50,
    disks: [{ mount: "D:", total_gb: 1000, used_gb: 500, fs: "NTFS" }],
    anomaly_count: 1,
    anomaly_categories: { cpu: 1, memory: 0, disk_io: 0, future_secret: "FUTURE-NESTED-SECRET" },
    future_runtime_secret: "FUTURE-RUNTIME-SECRET",
    health_deductions: [{ category: "cpu-sustained-load", points: 5 }],
    recommendation_categories: ["system-disk-low"],
    recommendation_count: 1,
  },
  notes: ["No process names or Wi-Fi credentials."],
};

const parsed = parseDiagnosticReport(JSON.stringify(original));
const safeDefault = redactDiagnosticReport(parsed, { includeDeviceIdentifiers: false, includeDetailedTelemetry: false });
assert.equal(safeDefault.system.host, undefined);
assert.equal(safeDefault.system.cpu_model, undefined);
assert.equal(safeDefault.system.cores, undefined);
assert.equal(safeDefault.runtime.gpu_name, undefined);
assert.equal(safeDefault.generated_at_utc, undefined);
assert.equal(safeDefault.runtime.network_today_rx_gb, undefined);
assert.equal(safeDefault.runtime.disks, undefined);
assert.equal(safeDefault.runtime.health_score, 91, "useful aggregate diagnostics remain available by default");
assert.equal(original.system.host, "LAPTOP-SECRET", "redaction must not mutate the in-memory source report");
assert.doesNotMatch(JSON.stringify(safeDefault), /LAPTOP-SECRET|CPU SECRET|GPU SECRET|network_today_rx_gb|FUTURE-/);
assert.equal(safeDefault.system.future_device_secret, undefined);
assert.equal(safeDefault.runtime.future_runtime_secret, undefined);
assert.equal(safeDefault.runtime.anomaly_categories.future_secret, undefined);

const fullOptIn = redactDiagnosticReport(parsed, { includeDeviceIdentifiers: true, includeDetailedTelemetry: true });
assert.equal(fullOptIn.system.host, "LAPTOP-SECRET");
assert.equal(fullOptIn.runtime.gpu_name, "GPU SECRET");
assert.equal(fullOptIn.runtime.disks[0].mount, "D:");
assert.equal(fullOptIn.generated_at_utc, original.generated_at_utc);

assert.throws(() => parseDiagnosticReport("not json"), /无法解析/);
assert.throws(() => parseDiagnosticReport(JSON.stringify({ schema_version: 9 })), /不受支持/);

const generateBody = dashboard.match(/const exportReport = async \(\) => \{([\s\S]*?)\n  \};/)?.[1];
const saveBody = dashboard.match(/const saveDiagnosticReport = async \(\) => \{([\s\S]*?)\n  \};/)?.[1];
assert.ok(generateBody && saveBody, "diagnostic generation and save must remain separate actions");
assert.match(generateBody, /parseDiagnosticReport\(await api\.exportDiagnosticReport\(\)\)/);
assert.doesNotMatch(generateBody, /\bsave\(|writeTextFile/, "generating a report must not open a file picker or write data");
assert.match(saveBody, /redactDiagnosticReport\(reportPreview/);
assert.ok(saveBody.indexOf("redactDiagnosticReport") < saveBody.indexOf("api.saveTextExport"), "redaction must precede file persistence");
assert.match(saveBody, /api\.saveTextExport\(filename,\s*JSON\.stringify\(report, null, 2\)\)/);
assert.match(dashboard, /const \[includeDeviceIdentifiers,[^\n]+\] = useState\(false\)/);
assert.match(dashboard, /const \[includeDetailedTelemetry,[^\n]+\] = useState\(false\)/);
assert.match(dashboard, /aria-labelledby="diagnostic-report-title"/);
assert.match(dashboard, /dialog\.showModal\(\)/, "preview must use the native modal dialog focus behavior");
assert.match(dashboard, /onCancel=\{\(event\) => \{\s*event\.preventDefault\(\);\s*if \(!exportingReport\) closeReportPreview\(\);/);
assert.match(dashboard, /reportPreviewJson/);
const previewMemo = dashboard.match(/const reportPreviewJson = useMemo\([\s\S]*?\n  \);/)?.[0];
assert.ok(previewMemo, "the potentially large redacted preview is memoized");
assert.match(previewMemo, /JSON\.stringify\(redactDiagnosticReport\(reportPreview/);
assert.match(previewMemo, /\[reportPreview, includeDeviceIdentifiers, includeDetailedTelemetry\]/);
assert.doesNotMatch(previewMemo, /\bstats\b|\bhist\b/, "telemetry rerenders do not re-redact and serialize the same preview");
assert.match(dashboard, /AeroTune 不会上传/);

console.log("PASS diagnostic preview defaults to redaction, requires opt-in for identifiers/details, and persists only previewed JSON");
