import assert from "node:assert/strict";
import fs from "node:fs";
import { cpuEvidence, diskEvidence, gpuEvidence, ioEvidence, validCapacityPair } from "../src/lib/resourceEvidence.js";

assert.equal(validCapacityPair(0, 8), true, "zero usage is valid when capacity is known");
assert.equal(validCapacityPair(8, 8), true, "fully used capacity is valid");
assert.equal(validCapacityPair(-1, 8), false, "negative usage is invalid");
assert.equal(validCapacityPair(9, 8), false, "usage above capacity is invalid");
assert.equal(validCapacityPair(0, 0), false, "zero capacity is unknown");
assert.equal(validCapacityPair(1, Number.NaN), false, "non-finite capacity is invalid");

const symptomGuide = fs.readFileSync("src/components/SymptomGuide.tsx", "utf8");
assert.match(symptomGuide, /validCapacityPair\(stats\.mem_used_gb, stats\.mem_total_gb\)/);
assert.match(symptomGuide, /validCapacityPair\(stats\.swap_used_gb, stats\.swap_total_gb\)/);
const statsHook = fs.readFileSync("src/hooks/useStats.ts", "utf8");
assert.match(statsHook, /if \(!sample\.sample_id \|\| !sample\.per_core\?\.length\)/, "valid non-memory telemetry is accepted when RAM capacity is missing");
assert.match(statsHook, /validCapacityPair\(sample\.mem_used_gb, sample\.mem_total_gb\)/, "invalid RAM is not inserted into usage history");
const dashboard = fs.readFileSync("src/pages/Dashboard.tsx", "utf8");
assert.match(dashboard, /validCapacityPair\(stats\.mem_used_gb, stats\.mem_total_gb\)/);
assert.match(dashboard, /\$\{fmtGb\(stats\.mem_used_gb\)\} 已用 \/ 总量 \$\{fmtGb\(stats\.mem_total_gb\)\}/, "dashboard distinguishes system-used memory from total capacity");
assert.match(dashboard, /候选分析时 Windows 可用 \$\{report\.available_before_mb\.toFixed\(0\)\} MB/, "manual available-memory snapshot is explicitly time-qualified");
const sidebar = fs.readFileSync("src/components/Sidebar.tsx", "utf8");
assert.match(sidebar, /validCapacityPair\(stats\.mem_used_gb, stats\.mem_total_gb\)/);
assert.match(sidebar, /aria-label="实时内存系统已用量与总量"/);
assert.match(sidebar, /已用 \/ 总量/, "persistent sidebar labels the numerator and denominator");
assert.match(symptomGuide, /系统报告内存已用[\s\S]*已用 \$\{memoryStats\.mem_used_gb\.toFixed\(1\)\} \/ 总量/, "memory diagnosis uses explicit system-used labels");
const gameMode = fs.readFileSync("src/pages/GameMode.tsx", "utf8");
const devMode = fs.readFileSync("src/pages/DevMode.tsx", "utf8");
assert.match(gameMode, /系统已用[\s\S]*mem_used_gb\.toFixed\(1\)\} \/ \$\{stats\.mem_total_gb\.toFixed\(0\)\} GB/, "game mode labels the system-used RAM figure");
assert.match(devMode, /系统已用[\s\S]*mem_used_gb\.toFixed\(1\)\} \/ \$\{stats\.mem_total_gb\.toFixed\(0\)\} GB/, "dev mode labels the system-used RAM figure");
const experimentMetrics = fs.readFileSync("src/lib/experimentMetrics.js", "utf8");
assert.match(experimentMetrics, /const memoryValid = validCapacityPair\(stats\.mem_used_gb, stats\.mem_total_gb\)/);
assert.match(experimentMetrics, /const gpuMemoryValid = validCapacityPair\(stats\.gpu\?\.mem_used_mb, stats\.gpu\?\.mem_total_mb\)/);

assert.deepEqual(gpuEvidence(null), [
  { label: "显卡遥测", value: "等待 GPU 遥测", measured: false, pending: true },
]);
assert.deepEqual(gpuEvidence({ gpu: null }), [
  { label: "显卡遥测", value: "当前未检测到显卡数据", measured: false },
]);
assert.deepEqual(gpuEvidence({ gpu: { utilization: 62.8, temperature: 87.2, mem_used_mb: 9000, mem_total_mb: 12000 } }), [
  { label: "GPU 利用率", value: "63%", measured: true },
  { label: "显卡温度", value: "87 °C", measured: true },
  { label: "显存用量", value: "75%（9000 / 12000 MB）", measured: true },
]);
assert.deepEqual(gpuEvidence({ gpu: { utilization: 105, temperature: -1, mem_used_mb: 900, mem_total_mb: 0 } }), [
  { label: "GPU 利用率", value: "未提供或超出有效范围", measured: false },
  { label: "显卡温度", value: "未提供或超出有效范围", measured: false },
  { label: "显存用量", value: "容量或使用量未提供", measured: false },
]);

assert.deepEqual(cpuEvidence(null), [
  { label: "CPU 基线", value: "诊断数据尚未提供", measured: false },
]);
const warmingDiagnostics = {
  process_sample_ready: true,
  cpu_baseline_warming_processes: 2,
  mem_baseline_warming_processes: 1,
  io_baseline_warming_processes: 3,
  anomalies: [],
};
assert.deepEqual(cpuEvidence(warmingDiagnostics), [
  { label: "CPU 基线", value: "2 个进程的 CPU 基线仍在建立；暂不作完整判断", measured: false, pending: true },
]);
assert.deepEqual(ioEvidence(null), [
  { label: "I/O 基线", value: "诊断数据尚未提供", measured: false },
]);
assert.deepEqual(ioEvidence({ process_sample_ready: false, anomalies: [] }), [
  { label: "I/O 基线", value: "等待首次进程采样", measured: false, pending: true },
]);
assert.deepEqual(ioEvidence({ ...warmingDiagnostics, anomalies: [] }), [
  { label: "I/O 基线", value: "3 个进程的 I/O 基线仍在建立；暂不作完整判断", measured: false, pending: true },
]);
assert.deepEqual(cpuEvidence({ ...warmingDiagnostics, cpu_baseline_warming_processes: 0, anomalies: [] }), [
  { label: "CPU 基线", value: "当前进程基线已就绪；未检出 CPU 异常", measured: true },
]);
assert.deepEqual(ioEvidence({ ...warmingDiagnostics, io_baseline_warming_processes: 0, anomalies: [] }), [
  { label: "异常 I/O 进程", value: "基线已就绪；当前未检出高于基线的异常进程", measured: true },
]);
assert.deepEqual(ioEvidence({ ...warmingDiagnostics, anomalies: null }), [
  { label: "I/O 基线", value: "3 个进程的 I/O 基线仍在建立；暂不作完整判断", measured: false, pending: true },
  { label: "I/O 诊断", value: "异常进程样本不可用", measured: false },
]);
const io = ioEvidence({ ...warmingDiagnostics, anomalies: [
  { pid: 82, name: "compiler.exe", kind: "io", value: 2048, baseline: 512, robust_z: 3.4 },
  { pid: 83, name: "bad.exe", kind: "io", value: Number.NaN, baseline: 0, robust_z: 1 },
  { pid: 84, name: "cpu.exe", kind: "cpu", value: 90, baseline: 10, robust_z: 4 },
  { pid: 85, name: "stale.exe", kind: "io", value: 512, baseline: 512, robust_z: 2.5 },
] });
assert.deepEqual(io, [
  { label: "compiler.exe（PID 82）", value: "2.0 MB/s · 基线 0.5 MB/s · 偏离 3.4 个稳健尺度", measured: true },
  { label: "I/O 基线", value: "3 个进程的 I/O 基线仍在建立；暂不作完整判断", measured: false, pending: true },
]);

assert.deepEqual(diskEvidence(null), [
  { label: "卷空间", value: "等待磁盘遥测", measured: false, pending: true },
]);
const disk = diskEvidence({
  system_drive: "c:\\",
  disks: [
    { mount: "D:", total_gb: 200, used_gb: 150 },
    { mount: "E:", total_gb: 300, used_gb: 100 },
    { mount: "F:", total_gb: 400, used_gb: 200 },
    { mount: "C:", total_gb: 100, used_gb: 90 },
  ],
});
assert.deepEqual(disk.slice(0, 3).map((row) => row.label), [
  "C: 已用 / 剩余",
  "D: 已用 / 剩余",
  "E: 已用 / 剩余",
]);
assert.match(disk[0].value, /剩余 10\.0 GB/);

console.log("Resource evidence passed: valid memory/swap capacities, baseline warm-up, GPU telemetry, and system-volume priority");
