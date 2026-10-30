function finite(value) {
  return typeof value === "number" && Number.isFinite(value);
}

export function validCapacityPair(used, total) {
  return finite(used) && finite(total) && total > 0 && used >= 0 && used <= total;
}

function percent(value) {
  return `${value.toFixed(0)}%`;
}

function processBaselineStatus(diagnostics, kind, label) {
  if (!diagnostics) return { state: "missing", row: { label, value: "诊断数据尚未提供", measured: false } };
  if (diagnostics.process_sample_ready !== true) {
    return { state: "waiting", row: { label, value: "等待首次进程采样", measured: false, pending: true } };
  }
  const warming = diagnostics[`${kind}_baseline_warming_processes`];
  if (!Number.isSafeInteger(warming) || warming < 0) {
    return { state: "unknown", row: { label, value: "基线覆盖状态不可用；暂不作完整判断", measured: false } };
  }
  if (warming > 0) {
    return {
      state: "warming",
      warming,
      row: { label, value: `${warming} 个进程的 ${kind === "io" ? "I/O" : "CPU"} 基线仍在建立；暂不作完整判断`, measured: false, pending: true },
    };
  }
  return { state: "ready", warming: 0, row: { label, value: "当前进程基线已就绪", measured: true } };
}

function validAnomalies(diagnostics, kind) {
  if (!Array.isArray(diagnostics?.anomalies)) return null;
  return diagnostics.anomalies.filter((item) => item
    && item.kind === kind
    && Number.isSafeInteger(item.pid)
    && item.pid > 0
    && typeof item.name === "string"
    && finite(item.value)
    && finite(item.baseline)
    && finite(item.robust_z)
    && item.value >= 0
    && item.baseline >= 0
    && item.value > item.baseline
    && item.robust_z > 0);
}

export function gpuEvidence(stats) {
  if (!stats) return [{ label: "显卡遥测", value: "等待 GPU 遥测", measured: false, pending: true }];
  const gpu = stats.gpu;
  if (!gpu) return [{ label: "显卡遥测", value: "当前未检测到显卡数据", measured: false }];

  const utilization = gpu.utilization;
  const temperature = gpu.temperature;
  const memoryUsed = gpu.mem_used_mb;
  const memoryTotal = gpu.mem_total_mb;
  const utilizationValid = finite(utilization) && utilization >= 0 && utilization <= 100;
  const temperatureValid = finite(temperature) && temperature >= 0 && temperature <= 130;
  const memoryValid = validCapacityPair(memoryUsed, memoryTotal);

  return [
    { label: "GPU 利用率", value: utilizationValid ? percent(utilization) : "未提供或超出有效范围", measured: utilizationValid },
    { label: "显卡温度", value: temperatureValid ? `${temperature.toFixed(0)} °C` : "未提供或超出有效范围", measured: temperatureValid },
    { label: "显存用量", value: memoryValid ? `${percent((memoryUsed / memoryTotal) * 100)}（${memoryUsed.toFixed(0)} / ${memoryTotal.toFixed(0)} MB）` : "容量或使用量未提供", measured: memoryValid },
  ];
}

export function cpuEvidence(diagnostics) {
  const baseline = processBaselineStatus(diagnostics, "cpu", "CPU 基线");
  if (!diagnostics) return [baseline.row];
  const anomalies = validAnomalies(diagnostics, "cpu");
  if (!anomalies) return [baseline.row, { label: "CPU 异常", value: "异常进程样本不可用", measured: false }];

  if (anomalies.length === 0) {
    return [baseline.state === "ready"
      ? { label: "CPU 基线", value: "当前进程基线已就绪；未检出 CPU 异常", measured: true }
      : baseline.row];
  }

  const names = anomalies.slice(0, 3).map((item) => `${item.name}（PID ${item.pid}）`).join("、");
  const statusRow = baseline.state === "ready"
    ? { label: "CPU 基线", value: "当前进程基线已就绪；下列异常已确认", measured: true }
    : baseline.row;
  return [statusRow, { label: "已确认 CPU 异常", value: names, measured: true }];
}

export function ioEvidence(diagnostics) {
  const baseline = processBaselineStatus(diagnostics, "io", "I/O 基线");
  if (!diagnostics) return [baseline.row];
  const anomalies = validAnomalies(diagnostics, "io");
  if (!anomalies) return [baseline.row, { label: "I/O 诊断", value: "异常进程样本不可用", measured: false }];

  if (anomalies.length === 0) {
    return [baseline.state === "ready"
      ? { label: "异常 I/O 进程", value: "基线已就绪；当前未检出高于基线的异常进程", measured: true }
      : baseline.row];
  }

  const rows = anomalies.slice(0, 3).map((item) => ({
    label: `${item.name.trim() || "未知进程"}（PID ${item.pid}）`,
    value: `${(item.value / 1024).toFixed(1)} MB/s · 基线 ${(item.baseline / 1024).toFixed(1)} MB/s · 偏离 ${item.robust_z.toFixed(1)} 个稳健尺度`,
    measured: true,
  }));
  if (baseline.state !== "ready") rows.push(baseline.row);
  return rows;
}

function driveRoot(value) {
  if (typeof value !== "string") return "";
  return /^\s*([a-z]:)/i.exec(value)?.[1].toUpperCase() ?? "";
}

export function diskEvidence(stats) {
  if (!stats) return [{ label: "卷空间", value: "等待磁盘遥测", measured: false, pending: true }];
  const disks = Array.isArray(stats.disks) ? stats.disks.filter((disk) => disk
    && validCapacityPair(disk.used_gb, disk.total_gb)) : [];
  if (disks.length === 0) {
    return [{ label: "卷空间", value: "当前没有有效磁盘容量数据", measured: false }];
  }

  const systemDrive = driveRoot(stats.system_drive);
  const systemIndex = systemDrive ? disks.findIndex((disk) => driveRoot(disk.mount) === systemDrive) : -1;
  const ordered = systemIndex >= 0
    ? [disks[systemIndex], ...disks.filter((_, index) => index !== systemIndex)]
    : disks;
  const rows = ordered.slice(0, 3).map((disk) => {
    const free = disk.total_gb - disk.used_gb;
    const used = (disk.used_gb / disk.total_gb) * 100;
    return {
      label: `${disk.mount} 已用 / 剩余`,
      value: `${used.toFixed(0)}% · ${disk.used_gb.toFixed(1)} / ${disk.total_gb.toFixed(1)} GB，剩余 ${free.toFixed(1)} GB`,
      measured: true,
    };
  });
  if (systemIndex < 0) {
    rows.unshift({
      label: "系统盘证据",
      value: systemDrive ? `未在当前磁盘样本中找到 ${systemDrive}` : "系统盘标识暂不可用；以下按采集顺序显示",
      measured: false,
    });
  }
  rows.push({ label: "大文件 / 目录排行", value: "当前遥测未提供；可在空间清理页扫描后预览", measured: false });
  return rows;
}
