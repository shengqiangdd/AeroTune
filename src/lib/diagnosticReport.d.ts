export interface DiagnosticReport {
  schema_version: 1;
  app_version: string;
  generated_at_utc?: string;
  generated_at_local?: string;
  system: {
    os: string;
    version: string;
    arch: string;
    host?: string;
    cpu_model?: string;
    cores?: number;
    admin: boolean;
  };
  runtime: {
    cpu_pct?: number;
    memory_used_gb?: number;
    memory_total_gb?: number;
    pagefile_used_gb?: number;
    pagefile_total_gb?: number;
    process_count?: number;
    uptime_secs?: number;
    sample_id?: number;
    health_score: number;
    cpu_frequency_mhz?: number;
    memory_pressure_pct?: number | null;
    pagefile_pressure_pct?: number | null;
    swap_in_use?: boolean;
    has_sample: boolean;
    network_rx_kbps?: number;
    network_tx_kbps?: number;
    network_today_rx_gb?: number;
    network_today_tx_gb?: number;
    battery_percent?: number | null;
    on_ac_power?: boolean | null;
    gpu_name?: string | null;
    gpu_utilization_pct?: number | null;
    gpu_temperature_c?: number | null;
    disks?: Array<{ mount: string; total_gb: number; used_gb: number; fs: string }>;
    anomaly_count: number;
    anomaly_categories: { cpu: number; memory: number; disk_io: number };
    health_deductions: Array<{ category: string; points: number }>;
    recommendation_categories: string[];
    recommendation_count: number;
  };
  notes: string[];
}

export interface DiagnosticReportOptions {
  includeDeviceIdentifiers: boolean;
  includeDetailedTelemetry: boolean;
}

export function parseDiagnosticReport(json: string): DiagnosticReport;
export function redactDiagnosticReport(
  report: DiagnosticReport,
  options: DiagnosticReportOptions,
): DiagnosticReport;
