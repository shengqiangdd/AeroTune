const SYSTEM_BASE_FIELDS = ["os", "version", "arch", "admin"];
const SYSTEM_IDENTIFIER_FIELDS = ["host", "cpu_model", "cores"];
const RUNTIME_SUMMARY_FIELDS = [
  "health_score",
  "has_sample",
  "anomaly_count",
  "anomaly_categories",
  "health_deductions",
  "recommendation_categories",
  "recommendation_count",
];
const RUNTIME_DETAIL_FIELDS = [
  "cpu_pct",
  "memory_used_gb",
  "memory_total_gb",
  "pagefile_used_gb",
  "pagefile_total_gb",
  "process_count",
  "uptime_secs",
  "sample_id",
  "cpu_frequency_mhz",
  "memory_pressure_pct",
  "pagefile_pressure_pct",
  "swap_in_use",
  "network_rx_kbps",
  "network_tx_kbps",
  "network_today_rx_gb",
  "network_today_tx_gb",
  "battery_percent",
  "on_ac_power",
  "gpu_utilization_pct",
  "gpu_temperature_c",
  "disks",
];

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function pick(source, fields) {
  const selected = {};
  for (const field of fields) {
    if (Object.hasOwn(source, field)) selected[field] = source[field];
  }
  return selected;
}

/**
 * Parse the versioned, local diagnostic payload before presenting any save action.
 * @param {string} json
 * @returns {import("./diagnosticReport.js").DiagnosticReport}
 */
export function parseDiagnosticReport(json) {
  let value;
  try {
    value = JSON.parse(json);
  } catch {
    throw new Error("诊断报告无法解析，请重试。");
  }
  if (
    !isRecord(value)
    || value.schema_version !== 1
    || typeof value.app_version !== "string"
    || !isRecord(value.system)
    || !isRecord(value.runtime)
    || !Array.isArray(value.notes)
  ) {
    throw new Error("诊断报告版本或字段不受支持，未保存任何文件。");
  }
  return value;
}

/**
 * Build a new allowlisted report containing only the fields selected in the preview.
 * Unknown top-level, system, runtime, and nested fields are intentionally discarded.
 * @param {import("./diagnosticReport.js").DiagnosticReport} report
 * @param {import("./diagnosticReport.js").DiagnosticReportOptions} options
 * @returns {import("./diagnosticReport.js").DiagnosticReport}
 */
export function redactDiagnosticReport(report, options) {
  const deviceFields = options.includeDeviceIdentifiers ? SYSTEM_IDENTIFIER_FIELDS : [];
  const runtimeFields = [
    ...RUNTIME_SUMMARY_FIELDS,
    ...(options.includeDetailedTelemetry ? RUNTIME_DETAIL_FIELDS : []),
    ...(options.includeDeviceIdentifiers ? ["gpu_name"] : []),
  ];
  const system = pick(report.system, [...SYSTEM_BASE_FIELDS, ...deviceFields]);
  const runtime = pick(report.runtime, runtimeFields);

  if (isRecord(runtime.anomaly_categories)) {
    runtime.anomaly_categories = pick(runtime.anomaly_categories, ["cpu", "memory", "disk_io"]);
  }
  if (Array.isArray(runtime.health_deductions)) {
    runtime.health_deductions = runtime.health_deductions
      .filter(isRecord)
      .map((item) => pick(item, ["category", "points"]));
  }
  if (Array.isArray(runtime.recommendation_categories)) {
    runtime.recommendation_categories = runtime.recommendation_categories
      .filter((item) => typeof item === "string");
  }
  if (Array.isArray(runtime.disks)) {
    runtime.disks = runtime.disks
      .filter(isRecord)
      .map((disk) => pick(disk, ["mount", "total_gb", "used_gb", "fs"]));
  }

  return {
    schema_version: 1,
    app_version: report.app_version,
    ...(options.includeDetailedTelemetry && typeof report.generated_at_utc === "string"
      ? { generated_at_utc: report.generated_at_utc }
      : {}),
    ...(options.includeDetailedTelemetry && typeof report.generated_at_local === "string"
      ? { generated_at_local: report.generated_at_local }
      : {}),
    system,
    runtime,
    notes: [
      "报告仅在本机预览和保存，不会自动上传。",
      options.includeDeviceIdentifiers
        ? "已选择包含主机名和硬件型号等设备标识。"
        : "已省略主机名、处理器/显卡型号和核心数。",
      options.includeDetailedTelemetry
        ? "已选择包含详细性能、网络、磁盘、电池遥测及精确时间。"
        : "已省略精确时间及详细性能、网络、磁盘、电池遥测。",
      "未包含进程名称/路径、Wi-Fi 密码或网络凭据。",
    ],
  };
}
