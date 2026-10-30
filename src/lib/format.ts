export function fmtBytes(kbps: number): string {
  // 输入 KB/s
  if (kbps >= 1024 * 1024) return (kbps / 1024 / 1024).toFixed(2) + " GB/s";
  if (kbps >= 1024) return (kbps / 1024).toFixed(2) + " MB/s";
  return kbps.toFixed(0) + " KB/s";
}

export function fmtFileSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = bytes;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit++;
  }
  return `${size.toFixed(unit === 0 ? 0 : 2)} ${units[unit]}`;
}

export function fmtGb(v: number): string {
  return v >= 10 ? v.toFixed(1) + " GB" : v.toFixed(2) + " GB";
}

export function fmtUptime(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (d > 0) return `${d} 天 ${h} 时`;
  if (h > 0) return `${h} 时 ${m} 分`;
  return `${m} 分`;
}

export function fmtTime(ts: number): string {
  const d = new Date(ts * 1000);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getMonth() + 1}-${d.getDate()} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
