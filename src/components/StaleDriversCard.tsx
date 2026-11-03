// 过期驱动包：被同名新版本取代的旧驱动包（DriverStore）。只读列出，逐个确认后删除；不可还原。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Cpu, RefreshCw } from "lucide-react";
import { api, StaleDriver, StaleDriverReport } from "../lib/api";
import { Btn, Card } from "./ui";
import { toastErr, toastOk } from "../lib/toast";

function sizeText(sizeMb: number | null) {
  return sizeMb === null ? "大小未知" : `${sizeMb.toFixed(1)} MB`;
}

export function StaleDriversCard() {
  const [report, setReport] = useState<StaleDriverReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const mounted = useRef(true);
  const requestId = useRef(0);

  const load = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true);
    setLoadError(null);
    try {
      const next = await api.listStaleDrivers();
      if (mounted.current && id === requestId.current) setReport(next);
    } catch (error) {
      if (mounted.current && id === requestId.current) setLoadError(String(error));
    } finally {
      if (mounted.current && id === requestId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => {
      mounted.current = false;
    };
  }, [load]);

  const rows = report?.drivers ?? [];
  const bigRows = useMemo(() => rows.filter((row) => (row.size_mb ?? 0) >= 1).length, [rows]);

  const remove = async (driver: StaleDriver) => {
    if (busy) return;
    const confirmed = confirm(
      `删除过期驱动包 ${driver.published}（${driver.original} ${driver.version}）？\n\n` +
        `它已被更新的 ${driver.superseded_by} 取代。释放约 ${sizeText(driver.size_mb)}。\n\n` +
        "此操作无法在 AeroTune 中还原：之后若需要，只能由 Windows 更新或厂商安装包重新提供。" +
        "仍被设备使用的驱动包会被 Windows 拒绝删除。",
    );
    if (!confirmed) return;
    setBusy(driver.published);
    try {
      await api.deleteStaleDriver(driver.published);
      toastOk(`已删除驱动包 ${driver.published}`);
    } catch (error) {
      toastErr(error);
    } finally {
      if (mounted.current) setBusy(null);
      void load();
    }
  };

  return (
    <Card
      className="xl:col-span-2 min-w-0"
      title="过期驱动包"
      icon={<Cpu size={15} className="text-amber-300" />}
      right={
        <Btn variant="ghost" onClick={() => void load()} disabled={loading || busy !== null}>
          <RefreshCw size={14} className={loading ? "animate-spin" : ""} /> {loading ? "读取中" : "刷新列表"}
        </Btn>
      }
    >
      <div className="mb-3 text-[12px] leading-5 text-slate-400">
        Windows 会把每个装过的第三方驱动包都留在驱动库里。当同一驱动（相同原始文件名与提供商）已有<b>版本更高</b>的包时，旧包就成为候选。
        版本相同、仅日期不同的包不会被列出。删除需要管理员权限，不使用强制参数：仍被设备使用的包由 Windows 拒绝。<b>删除无法还原。</b>
      </div>
      {loadError && (
        <div role="alert" className="mb-3 rounded-lg border border-rose-400/20 bg-rose-400/5 px-3 py-2 text-[12px] text-rose-200">
          读取失败，未列出任何驱动：{loadError}
        </div>
      )}
      {loading && report === null && <div className="h-16 animate-pulse rounded-lg bg-white/4" />}
      {report !== null && !loadError && (
        <>
          <div role="status" className="mb-3 text-[12px] text-slate-300">
            共 {report.total_packages} 个第三方驱动包，其中 {rows.length} 个已被更新版本取代
            {rows.length > 0 && <>；可测得约 {report.measured_mb.toFixed(1)} MB{report.unmeasured > 0 && `（另有 ${report.unmeasured} 个大小未知）`}</>}。
            {rows.length > 0 && bigRows < rows.length && <span className="text-slate-500"> 其余多为几百 KB 的小包，收益很小。</span>}
          </div>
          {rows.length === 0 && (
            <div className="rounded-lg border border-white/6 bg-white/3 px-4 py-5 text-center text-[13px] text-slate-300">
              没有被更新版本取代的驱动包
            </div>
          )}
          {rows.length > 0 && (
            <div className="max-h-[min(28rem,50vh)] space-y-2 overflow-y-auto overscroll-contain pr-1">
              {[...rows]
                .sort((a, b) => (b.size_mb ?? -1) - (a.size_mb ?? -1))
                .map((driver) => (
                  <div key={driver.published} className="flex min-w-0 flex-wrap items-start gap-3 rounded-xl border border-white/6 bg-white/3 px-3.5 py-3 sm:flex-nowrap">
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-[13px] font-medium text-slate-100" title={driver.original}>{driver.original}</div>
                      <div className="mt-0.5 text-[11px] text-slate-400">
                        {driver.provider || "未知提供商"} · {driver.class_name || "未知类别"} · {driver.published}
                      </div>
                      <div className="mt-1 text-[11px] text-slate-500">
                        {driver.version}（{driver.date}）→ 已被 {driver.superseded_by} 取代
                      </div>
                    </div>
                    <div className="shrink-0 text-right text-[12px] tabular-nums text-slate-300">{sizeText(driver.size_mb)}</div>
                    <Btn variant="danger" disabled={busy !== null || loading} onClick={() => void remove(driver)} className="ml-auto shrink-0">
                      {busy === driver.published ? "删除中…" : "删除"}
                    </Btn>
                  </div>
                ))}
            </div>
          )}
        </>
      )}
    </Card>
  );
}
