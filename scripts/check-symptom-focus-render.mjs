import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";

const server = await createServer({
  configFile: "./vite.config.ts",
  appType: "custom",
  logLevel: "error",
  optimizeDeps: { noDiscovery: true, include: [] },
  server: { middlewareMode: true },
});

try {
  const { SymptomGuide } = await server.ssrLoadModule("/src/components/SymptomGuide.tsx");
  const render = (props) => renderToStaticMarkup(React.createElement(SymptomGuide, props));
  const common = {
    diagnostics: null,
    recommendationsLoading: false,
    recommendationsError: false,
    onNav: () => {},
  };

  const gpuMarkup = render({
    ...common,
    stats: { gpu: { utilization: 48, temperature: 87, mem_used_mb: 9000, mem_total_mb: 12000 } },
    recommendations: [{ id: "gpu-temperature-high", title: "留意显卡温度", reason: "显卡温度约 87°C", action: "processes", priority: 82 }],
  });
  assert.match(gpuMarkup, /显卡温度 \/ 显存 · 当前证据/);
  assert.match(gpuMarkup, /GPU 利用率/);
  assert.match(gpuMarkup, /显卡温度/);
  assert.match(gpuMarkup, /显存用量/);
  assert.match(gpuMarkup, /role="switch"/);
  assert.match(gpuMarkup, /aria-label="自动跟随最高优先级建议"/);
  assert.match(gpuMarkup, /aria-checked="true"/);
  assert.match(gpuMarkup, /aria-live="polite"/);

  const cpuWarmupMarkup = render({
    ...common,
    stats: { cpu: 95 },
    diagnostics: { process_sample_ready: true, cpu_baseline_warming_processes: 4, anomalies: [] },
    recommendations: [{ id: "process-17-cpu", title: "检查高 CPU 进程", reason: "滚动基线仍在建立", action: "processes", priority: 84 }],
  });
  assert.match(cpuWarmupMarkup, /4 个进程的 CPU 基线仍在建立；暂不作完整判断/);
  assert.match(cpuWarmupMarkup, /采样中/);
  assert.match(cpuWarmupMarkup, /id="process-baseline-live-status" class="sr-only" role="status" aria-live="polite" aria-atomic="true"/);
  assert.doesNotMatch(cpuWarmupMarkup, /当前未检出 CPU 异常/);

  const ioMarkup = render({
    ...common,
    stats: null,
    diagnostics: { process_sample_ready: true, io_baseline_warming_processes: 0, anomalies: [{ pid: 82, name: "compiler.exe", kind: "io", value: 2048, baseline: 512, robust_z: 3.4 }] },
    recommendations: [{ id: "process-82-io", title: "检查异常进程", reason: "高于滚动基线", action: "processes", priority: 70 }],
  });
  assert.match(ioMarkup, /磁盘 I\/O · 当前证据/);
  assert.match(ioMarkup, /compiler\.exe（PID 82）/);
  assert.match(ioMarkup, /2\.0 MB\/s · 基线 0\.5 MB\/s/);

  const ioWarmupMarkup = render({
    ...common,
    stats: null,
    diagnostics: { process_sample_ready: true, io_baseline_warming_processes: 2, anomalies: [] },
    recommendations: [{ id: "process-83-io", title: "检查异常进程", reason: "建立滚动基线", action: "processes", priority: 70 }],
  });
  assert.match(ioWarmupMarkup, /2 个进程的 I\/O 基线仍在建立；暂不作完整判断/);
  assert.doesNotMatch(ioWarmupMarkup, /当前未检出高于基线的异常进程/);

  const startupMarkup = render({
    ...common,
    stats: null,
    diagnostics: { deductions: [{ item: "启动项过多", cost: 5 }] },
    recommendations: [{ id: "startup-burden", title: "检查启动项", reason: "包含已禁用项", action: "startup", priority: 55 }],
  });
  assert.match(startupMarkup, /启动项检查 · 当前证据/);
  assert.doesNotMatch(startupMarkup, /开机慢/);
  assert.match(startupMarkup, /统计的启动项数量偏多，计数可能包含已禁用项/);
  assert.match(startupMarkup, /尚无启动耗时数据，因此此建议仅提示检查项目数量，不代表启动变慢/);

  const diskMarkup = render({
    ...common,
    stats: { system_drive: "C:", disks: [
      { mount: "D:", total_gb: 200, used_gb: 150 },
      { mount: "E:", total_gb: 300, used_gb: 100 },
      { mount: "F:", total_gb: 400, used_gb: 200 },
      { mount: "C:", total_gb: 100, used_gb: 90 },
    ] },
    recommendations: [{ id: "system-disk-low", title: "清理系统盘空间", reason: "系统盘 C: 剩余偏少", action: "cleaner", priority: 80 }],
  });
  const systemDisk = diskMarkup.indexOf("C: 已用 / 剩余");
  const dataDisk = diskMarkup.indexOf("D: 已用 / 剩余");
  assert.ok(systemDisk >= 0 && dataDisk > systemDisk, "system-drive evidence should lead enumeration order");

  console.log("Symptom focus render passed: priority focus, warm-up truth, startup semantics, system-disk evidence, and accessibility");
} finally {
  await server.close();
}
