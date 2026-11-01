// 设置：管理员提权 / 变更日志与还原 / 开机自启 / 关于
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ShieldCheck, Database,
  ShieldAlert,
  History,
  Undo2,
  Trash2,
  FolderOpen,
  Power,
  Info,
  Monitor,
  Volume2,
  Wifi,
  HardDrive,
  Rocket,
  Bluetooth,
  AppWindow,
  MoonStar,
  RefreshCw,
} from "lucide-react";
import { api, AvailableUpdate, Change, SysInfo, ShadowStore, UpdateReadiness, WingetQueryResult } from "../lib/api";
import { Btn, PageHead, Card, Chip, Toggle } from "../components/ui";
import { toastOk, toastErr, toast } from "../lib/toast";
import { fmtTime } from "../lib/format";
import { buildChangeJournalCsv, buildChangeJournalJson, filterChanges, getChangeCategory, getChangeRevertTarget, summarizeUndoTargets } from "../lib/changeJournal.js";
import type { ChangeCategoryFilter, ChangeReversibilityFilter } from "../lib/changeJournal.js";
import { clearLocalObservationHistory, LOCAL_OBSERVATION_HISTORY_TARGETS } from "../lib/localObservationHistory.js";
import type { LocalObservationHistoryId, LocalObservationHistoryResult } from "../lib/localObservationHistory.js";

type SettingsProps = {
  admin: boolean;
  onAdmin: (v: boolean) => void;
  appVersion: string;
  requestedSection?: string | null;
  onRequestedSectionHandled?: (found: boolean) => void;
};

export function Settings({ admin, onAdmin, appVersion, requestedSection, onRequestedSectionHandled }: SettingsProps) {
  const [changes, setChanges] = useState<Change[]>([]);
  const [changesStatus, setChangesStatus] = useState<"loading" | "ready" | "error">("loading");
  const [changesError, setChangesError] = useState<string | null>(null);
  const changesRequestId = useRef(0);
  const [journalQuery, setJournalQuery] = useState("");
  const [journalCategory, setJournalCategory] = useState<ChangeCategoryFilter>("all");
  const [journalReversibility, setJournalReversibility] = useState<ChangeReversibilityFilter>("all");
  const [journalExporting, setJournalExporting] = useState(false);
  const [info, setInfo] = useState<SysInfo | null>(null);
  const [infoStatus, setInfoStatus] = useState<"loading" | "ready" | "error">("loading");
  const [localHistoryResults, setLocalHistoryResults] = useState<LocalObservationHistoryResult[] | null>(null);
  const localHistoryCleanupPending = useRef(false);
  const [autostart, setAutostart] = useState<boolean | null>(null);
  const [closeToTray, setCloseToTray] = useState<boolean | null>(null);
  const [autoRestore, setAutoRestore] = useState<boolean | null>(null);
  const [creating, setCreating] = useState(false);
  const [telemetry, setTelemetry] = useState<boolean | null>(null);
  const [settingsStatus, setSettingsStatus] = useState<"loading" | "ready" | "error">("loading");
  const settingsRequestId = useRef(0);
  const [settingsLoadWarning, setSettingsLoadWarning] = useState<string | null>(null);
  const [importingConfig, setImportingConfig] = useState(false);
  const [shadows, setShadows] = useState<ShadowStore[]>([]);
  const [shadowsStatus, setShadowsStatus] = useState<"loading" | "ready" | "error">("loading");
  const [openingSettings, setOpeningSettings] = useState<string | null>(null);
  const openingSettingsRef = useRef(false);
  const [updateReadiness, setUpdateReadiness] = useState<UpdateReadiness | null>(null);
  const [updateReadinessError, setUpdateReadinessError] = useState<string | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);
  const [availableUpdate, setAvailableUpdate] = useState<AvailableUpdate | null>(null);
  const [checkingWinget, setCheckingWinget] = useState(false);
  const [wingetResult, setWingetResult] = useState<WingetQueryResult | null>(null);
  const [wingetError, setWingetError] = useState<string | null>(null);

  useEffect(() => {
    if (!requestedSection) return;
    let focusFrame: number | null = null;
    const scrollFrame = requestAnimationFrame(() => {
      // Let the route-level main landmark receive focus first; then move focus to this section.
      focusFrame = requestAnimationFrame(() => {
        const target = document.getElementById(requestedSection);
        if (!target) {
          onRequestedSectionHandled?.(false);
          return;
        }
        target.scrollIntoView({ behavior: "smooth", block: "start" });
        target.focus({ preventScroll: true });
        onRequestedSectionHandled?.(true);
      });
    });
    return () => {
      cancelAnimationFrame(scrollFrame);
      if (focusFrame !== null) cancelAnimationFrame(focusFrame);
    };
  }, [requestedSection, onRequestedSectionHandled]);

  const load = useCallback(async () => {
    const requestId = ++changesRequestId.current;
    setChangesStatus("loading");
    setChangesError(null);
    try {
      const result = await api.listChanges();
      if (requestId === changesRequestId.current) { setChanges(result); setChangesStatus("ready"); }
    } catch (error) {
      if (requestId === changesRequestId.current) {
        setChangesStatus("error");
        setChangesError(error instanceof Error ? error.message : String(error));
      }
    }
  }, []);

  const loadSettings = useCallback(async () => {
    const requestId = ++settingsRequestId.current;
    setSettingsStatus("loading");
    setSettingsLoadWarning(null);
    const [autostartResult, autoSettingsResult, telemetryResult] = await Promise.allSettled([
      api.getAutostart(),
      api.getAutoSettings(),
      api.getTelemetryMinimal(),
    ]);
    if (requestId !== settingsRequestId.current) return;
    let failed = false;
    if (autostartResult.status === "fulfilled") setAutostart(autostartResult.value);
    else { setAutostart(null); failed = true; }
    if (autoSettingsResult.status === "fulfilled") {
      setCloseToTray(autoSettingsResult.value.close_to_tray);
      setAutoRestore(autoSettingsResult.value.auto_restore_point);
      setSettingsLoadWarning(autoSettingsResult.value.load_warning ?? null);
    } else { setCloseToTray(null); setAutoRestore(null); failed = true; }
    if (telemetryResult.status === "fulfilled") setTelemetry(telemetryResult.value);
    else { setTelemetry(null); failed = true; }
    setSettingsStatus(failed ? "error" : "ready");
  }, []);

  const importConfigFile = useCallback(async () => {
    if (importingConfig) return;
    setImportingConfig(true);
    try {
      const imported = await api.importConfig();
      if (!imported) return;
      toastOk("配置已导入并生效");
      await loadSettings();
    } catch (error) {
      toastErr(error);
      await loadSettings();
    } finally {
      setImportingConfig(false);
    }
  }, [importingConfig, loadSettings]);

  const loadShadows = useCallback(async () => {
    setShadowsStatus("loading");
    try {
      setShadows(await api.listShadowStorage());
      setShadowsStatus("ready");
    } catch {
      setShadowsStatus("error");
    }
  }, []);

  const loadInfo = useCallback(async () => {
    setInfoStatus("loading");
    try {
      setInfo(await api.getSystemInfo());
      setInfoStatus("ready");
    } catch {
      setInfoStatus("error");
    }
  }, []);

  const loadUpdateReadiness = useCallback(async () => {
    setUpdateReadinessError(null);
    try {
      setUpdateReadiness(await api.getSignedUpdateReadiness());
    } catch (error) {
      setUpdateReadiness(null);
      setUpdateReadinessError(String(error));
    }
  }, []);

  const checkForSignedUpdate = async () => {
    if (!updateReadiness?.enabled || checkingUpdate) return;
    setCheckingUpdate(true);
    try {
      const result = await api.checkSignedUpdate();
      setAvailableUpdate(result);
      toastOk(result ? `检测到新版本 v${result.available_version}` : "当前已是最新版本");
    } catch (error) {
      toastErr(error);
    } finally {
      setCheckingUpdate(false);
    }
  };

  const installSignedUpdate = async () => {
    if (!availableUpdate || installingUpdate) return;
    const confirmed = window.confirm(
      `将从 GitHub Releases 下载并验证 v${availableUpdate.available_version}，然后退出应用并启动 Windows 安装器。此安装器会请求 UAC 管理员确认；不会静默提权。是否继续？`,
    );
    if (!confirmed) return;
    setInstallingUpdate(true);
    try {
      await api.requestSignedUpdate(availableUpdate.available_version);
    } catch (error) {
      toastErr(error);
      setInstallingUpdate(false);
    }
  };

  const checkWingetUpdates = async () => {
    if (checkingWinget) return;
    const confirmed = window.confirm(
      "将通过 Windows App Installer 中的 WinGet 执行只读的“list --upgrade-available”查询。该操作可能联网、更新源缓存，并按 WinGet 行为向已配置软件源发送查询或包标识；不会自动接受源协议，也不会安装或升级软件。是否继续？",
    );
    if (!confirmed) return;
    setCheckingWinget(true);
    setWingetResult(null);
    setWingetError(null);
    try {
      setWingetResult(await api.checkWingetUpdates());
    } catch (error) {
      setWingetError(String(error));
    } finally {
      setCheckingWinget(false);
    }
  };

  const filteredChanges = useMemo(
    () => filterChanges(changes, journalQuery, journalCategory, journalReversibility).slice().reverse(),
    [changes, journalQuery, journalCategory, journalReversibility],
  );

  const exportJournal = async (format: "csv" | "json") => {
    if (journalExporting || filteredChanges.length === 0) return;
    setJournalExporting(true);
    try {
      const date = new Date().toISOString().slice(0, 10);
      const filename = `AeroTune-change-journal-${date}.${format}`;
      const content = format === "csv"
        ? buildChangeJournalCsv(filteredChanges, appVersion)
        : buildChangeJournalJson(filteredChanges, appVersion);
      const saved = await api.saveTextExport(filename, content);
      if (!saved) return;
      toastOk(`已导出 ${filteredChanges.length} 条变更摘要`);
    } catch (error) {
      toastErr(error);
    } finally {
      setJournalExporting(false);
    }
  };

  const runLocalHistoryCleanup = (retryFailed = false) => {
    if (localHistoryCleanupPending.current) return;
    const previousResults = localHistoryResults ?? [];
    const targetIds: LocalObservationHistoryId[] = retryFailed
      ? previousResults.filter((result) => !result.ok).map((result) => result.id)
      : LOCAL_OBSERVATION_HISTORY_TARGETS.map((target) => target.id);
    if (targetIds.length === 0) return;
    if (!retryFailed) {
      const labels = targetIds.map((id) => LOCAL_OBSERVATION_HISTORY_TARGETS.find((target) => target.id === id)?.label ?? id);
      const confirmed = window.confirm(
        `只尝试删除 AeroTune WebView localStorage 中以下 ${labels.length} 类历史：\n${labels.join("\n")}\n\n不会删除设置、变更日志/撤销快照、启动或计划恢复备份、日志及其他 %APPDATA% 文件；快速导航记录和可恢复的实验会话草稿也会保留。删除不可撤销，确认继续？`,
      );
      if (!confirmed) return;
    }

    localHistoryCleanupPending.current = true;
    let attempts: LocalObservationHistoryResult[];
    try {
      attempts = clearLocalObservationHistory(targetIds);
    } catch {
      attempts = targetIds.map((id) => ({
        id,
        label: LOCAL_OBSERVATION_HISTORY_TARGETS.find((target) => target.id === id)?.label ?? id,
        ok: false,
      }));
    } finally {
      localHistoryCleanupPending.current = false;
    }

    const resultsById = new Map((retryFailed ? previousResults : []).map((result) => [result.id, result]));
    for (const attempt of attempts) resultsById.set(attempt.id, attempt);
    const results = LOCAL_OBSERVATION_HISTORY_TARGETS.flatMap((target) => {
      const result = resultsById.get(target.id);
      return result ? [result] : [];
    });
    setLocalHistoryResults(results);
    const failed = results.filter((result) => !result.ok);
    if (failed.length) {
      toastErr(`仍有 ${failed.length} 类本机历史未能清除；失败项已保留，可单独重试。`);
    } else {
      toastOk(retryFailed ? "未完成的本机历史清理已重试成功" : "已处理所列本机历史；不存在的记录安全忽略");
    }
  };

  useEffect(() => {
    load();
    void loadInfo();
    void loadSettings();
    void loadShadows();
    void loadUpdateReadiness();
  }, [load, loadInfo, loadSettings, loadShadows, loadUpdateReadiness]);

  const elevate = async () => {
    try {
      await api.relaunchAsAdmin();
    } catch (e) {
      toastErr(e);
    }
  };

  const confirmUndoTargets = (items: Change[], action: string) => {
    const summary = summarizeUndoTargets(items, 20);
    if (summary.total === 0) return false;
    if (summary.tooMany) {
      toastErr(`本次操作包含 ${summary.total} 个目标，超过确认框可完整展示的 20 项；请筛选或逐条还原。`);
      return false;
    }
    if (summary.tooLong) {
      toastErr("本次确认目标内容超过安全展示上限；请筛选或逐条还原。");
      return false;
    }
    if (summary.invalidTarget) {
      toastErr("恢复目标信息缺失、过长或含不可见控制字符，无法安全完整确认；未执行还原。");
      return false;
    }
    return window.confirm(
      `${action}\n\n实际还原目标：\n${summary.visible.join("\n")}\n\n请核对目标；若路径、接口或设置不符合预期，请取消。`,
    );
  };

  const revertOne = async (c: Change) => {
    if (changesStatus !== "ready") return;
    if (!confirmUndoTargets([c], "即将根据本地恢复快照还原此记录。")) return;
    try {
      await api.revertChange(c);
      toastOk(`已还原：${c.desc}`);
      load();
    } catch (e) {
      toastErr(e);
      void load();
    }
  };

  const revertGroup = async (groupId: number, _label: string, count: number) => {
    if (changesStatus !== "ready") return;
    const groupChanges = changes.filter((change) => change.group_id === groupId && change.revert.t !== "none");
    if (groupChanges.length !== count) {
      toastErr("恢复组记录已变化，请刷新日志后重试。");
      load();
      return;
    }
    if (!confirmUndoTargets(groupChanges, `按逆序尝试还原此组中的 ${count} 条设置。`)) return;
    try {
      const restored = await api.revertChangeGroup(groupId, groupChanges);
      toastOk(`恢复胶囊已还原 ${restored} 条设置`);
    } catch (error) {
      toastErr(error);
    } finally {
      load();
    }
  };

  const revertAll = async () => {
    if (changesStatus !== "ready") return;
    const reversibleChanges = changes.filter((change) => change.revert.t !== "none");
    if (!confirmUndoTargets(reversibleChanges, `按逆序还原 ${reversibleChanges.length} 条支持还原的变更。`)) return;
    try {
      const n = await api.revertAll(reversibleChanges);
      toastOk(`已还原 ${n} 条变更`);
    } catch (e) {
      toastErr(e);
    } finally {
      load();
    }
  };

  const toggleAutostart = async (v: boolean) => {
    try {
      await api.setAutostart(v);
      setAutostart(await api.getAutostart());
      toastOk(v ? "已开启开机自启（静默驻留托盘）" : "已关闭开机自启");
    } catch (e) {
      toastErr(e);
    }
  };

  const toggleTray = async (v: boolean) => {
    try {
      await api.setAutoSetting("close_to_tray", v);
      setCloseToTray(v);
      toastOk(v ? "关闭窗口时将驻留托盘" : "关闭窗口将直接退出");
    } catch (e) {
      toastErr(e);
    }
  };

  const createRestore = async () => {
    setCreating(true);
    toast("info", "正在创建还原点（约 10-30 秒）…");
    try {
      await api.createRestorePoint("AeroTune 手动还原点");
      toastOk("系统还原点已创建");
    } catch (e) {
      toastErr(e);
    }
    setCreating(false);
  };

  const toggleTelemetry = async (v: boolean) => {
    try {
      await api.setTelemetryMinimal(v);
      setTelemetry(v);
      toastOk(v ? "遥测已降至最低（DiagTrack 已停止）" : "遥测已恢复默认");
    } catch (e) {
      toastErr(e);
    }
  };

  const delOldest = async (drive: string) => {
    if (!confirm(`删除 ${drive} 盘最旧的还原点？`)) return;
    try {
      await api.deleteOldestShadow(drive);
      toastOk("已删除最旧还原点");
      void loadShadows();
    } catch (e) {
      toastErr(e);
    }
  };

  const capShadow = async (drive: string, gb: number) => {
    try {
      await api.setShadowMax(drive, gb);
      toastOk(`${drive} 盘还原点配额 → ${gb}GB`);
      void loadShadows();
    } catch (e) {
      toastErr(e);
    }
  };

  const toggleAutoRestore = async (v: boolean) => {
    try {
      await api.setAutoSetting("auto_restore_point", v);
      setAutoRestore(v);
      toastOk(v ? "重大调优（HAGS/服务）前将自动创建还原点" : "已关闭自动还原点");
    } catch (e) {
      toastErr(e);
    }
  };

  const recoveryGroups = useMemo(() => {
    const groupMap = new Map<number, { id: number; label: string; count: number; ts: number }>();
    for (const change of changes) {
      const id = change.group_id;
      if (id == null || change.revert.t === "none") continue;
      const group = groupMap.get(id);
      if (group) {
        group.count += 1;
        group.ts = Math.max(group.ts, change.ts);
      } else {
        groupMap.set(id, { id, label: change.group_label || "AeroTune 操作组", count: 1, ts: change.ts });
      }
    }
    return [...groupMap.values()].sort((a, b) => b.ts - a.ts);
  }, [changes]);

  return (
    <div>
      <PageHead title="设置" desc="权限管理、变更日志、开机自启与关于" />
      {settingsStatus === "loading" && (
        <div role="status" className="mb-4 rounded-lg border border-white/8 bg-white/3 px-3 py-2 text-xs text-slate-400">正在读取开机自启、托盘、遥测与自动还原设置…</div>
      )}
      {settingsStatus === "error" && (
        <div role="alert" className="mb-4 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-xs text-amber-100">
          <span>部分设置读取失败；未读取项显示“未读取”，相关开关暂不可操作。</span>
          <Btn variant="ghost" onClick={loadSettings}>重试读取</Btn>
        </div>
      )}
      {settingsLoadWarning && (
        <div className="mb-4 flex min-w-0 flex-wrap items-start gap-2 rounded-lg border border-amber-300/25 bg-amber-300/6 px-3 py-2 text-xs text-amber-100">
          <div role="alert" className="flex min-w-0 flex-1 items-start gap-2">
            <ShieldAlert size={14} aria-hidden="true" className="mt-0.5 shrink-0" />
            <span className="min-w-0 break-words">{settingsLoadWarning}</span>
          </div>
          <Btn variant="ghost" onClick={importConfigFile} disabled={importingConfig} className="shrink-0">
            {importingConfig ? "正在导入…" : "重新导入配置…"}
          </Btn>
        </div>
      )}

      <div className="mb-5 grid grid-cols-1 gap-4 xl:grid-cols-2">
        <Card
          title="管理员权限"
          icon={admin ? <ShieldCheck size={15} className="text-emerald-300" /> : <ShieldAlert size={15} className="text-amber-300" />}
        >
          <div className="flex items-center justify-between gap-4">
            <div className="text-[13px] text-slate-400">
              {admin
                ? "当前以管理员身份运行，所有调优功能可用。"
                : "部分系统级调优（HAGS / 前台响应优先 / HKLM 启动项 / 系统目录清理 / TCP 调优）需要管理员权限。"}
            </div>
            {!admin && (
              <Btn onClick={elevate} className="shrink-0">
                <ShieldCheck size={14} /> 提权重启
              </Btn>
            )}
            {admin && <Chip tone="green">已提权</Chip>}
          </div>
        </Card>

        <Card title="开机自启" icon={<Power size={15} className="text-cyan-300" />}>
          <div className="flex items-center justify-between">
            <div className="pr-4 text-[13px] text-slate-400">
              开机静默驻留托盘（--silent），不弹窗口不拖慢开机
            </div>
            {autostart === null ? <Chip tone={settingsStatus === "loading" ? "slate" : "amber"}>{settingsStatus === "loading" ? "读取中…" : "未读取"}</Chip> : <Toggle label="开机自启" on={autostart} onChange={toggleAutostart} disabled={settingsStatus === "loading"} />}
          </div>
          <div className="mt-4 flex items-center justify-between border-t border-white/6 pt-4">
            <div className="pr-4 text-[13px] text-slate-400">
              点击窗口 × 时最小化到托盘继续守护（托盘右键可退出）
            </div>
            <div className="flex items-center gap-2">
              <Chip tone={closeToTray === null ? (settingsStatus === "loading" ? "slate" : "amber") : closeToTray ? "cyan" : "slate"}>{closeToTray === null ? (settingsStatus === "loading" ? "读取中…" : "状态未知") : closeToTray ? "驻留" : "直接退出"}</Chip>
              {closeToTray !== null && <Toggle label="关闭窗口时驻留托盘" on={closeToTray} onChange={toggleTray} disabled={settingsStatus === "loading"} />}
            </div>
          </div>
        </Card>

        <Card title="遥测治理" icon={<Power size={15} className="text-rose-300" />}>
          <div className="flex items-center justify-between">
            <div className="pr-4 text-[13px] text-slate-400">
              遥测降至最低并停用 DiagTrack（后台诊断跟踪服务）— 隐私与后台资源双收益
            </div>
            {telemetry === null ? <Chip tone={settingsStatus === "loading" ? "slate" : "amber"}>{settingsStatus === "loading" ? "读取中…" : "未读取"}</Chip> : <Toggle label="遥测治理" on={telemetry} onChange={toggleTelemetry} disabled={settingsStatus === "loading"} />}
          </div>
        </Card>

        <Card title="配置备份" icon={<Database size={15} className="text-violet-300" />}>
          <div className="mb-3 text-[13px] text-slate-400">
            仅导出/导入自动化配置（引擎设置、名单与规则），不是完整数据备份；数据保留说明见下方
          </div>
          <div className="flex gap-2">
            <Btn
              variant="ghost"
              onClick={async () => {
                try {
                  const p = await api.exportConfig();
                  toastOk(`已导出到桌面：${p.split("\\").pop()}`);
                } catch (e) {
                  toastErr(e);
                }
              }}
            >
              导出到桌面
            </Btn>
            <Btn variant="ghost" onClick={importConfigFile} disabled={importingConfig}>
              {importingConfig ? "正在导入…" : "导入配置…"}
            </Btn>
          </div>
        </Card>

        <Card id="settings-local-data" focusableTarget title="本机数据与隐私说明" icon={<ShieldCheck size={15} className="text-emerald-300" />}>
          <p className="text-[12px] leading-5 text-slate-400">诊断报告不会自动上传；网络质量检测只在你手动开始时发送 ICMP 探测。更新服务当前未启用。本页内容依据当前源码，尚待产品/法律审阅，不是正式隐私声明。</p>
          <details className="mt-3 rounded-lg border border-white/8 bg-white/3 px-3 py-2">
            <summary className="cursor-pointer text-[12px] font-medium text-cyan-200">查看本机存储、备份和卸载边界</summary>
            <div className="mt-2 space-y-2 text-[11px] leading-5 text-slate-400">
              <p>程序设置、变更日志与撤销快照、延迟启动恢复记录、计划备份、屏幕亮度恢复快照和日志保存在当前 Windows 用户的 Roaming AppData 下（通常为 %APPDATA%\AeroTune）。亮度快照只记录原始/自动变暗的百分比，恢复完成或实时亮度已与快照状态不一致后清除；这些本机数据未由 AeroTune 另行加密。</p>
              <p>各页面的网络、磁盘、进程和电池历史保存在 WebView 本机存储，可能与上述目录分开；进程与电池历史需用户开启。诊断报告会先显示最终预览，设备标识和详细指标默认排除，分别勾选后才会包含；导出由你选择保存位置，不会自动发送。</p>
              <p>配置导出仅包含引擎设置、名单和规则；不包含变更日志、撤销快照、屏幕亮度恢复快照、启动/计划恢复备份或各页本地历史，因此不是完整数据备份。</p>
              <p className="rounded-md border border-amber-300/15 bg-amber-300/5 px-2.5 py-2 text-amber-100">目前没有“清空全部本机数据”功能；下方操作仅清理明确列出的 WebView 历史。卸载时用户数据保留或删除行为尚未在一次性 Windows 虚拟机中验证。卸载前如需保留撤销/恢复记录，请关闭 AeroTune 后备份 Roaming AppData 下的 AeroTune 目录；WebView 历史可能不在该目录内，不能把这份副本视为完整备份。</p>
            </div>
          </details>
          <details className="mt-2 rounded-lg border border-white/8 bg-white/3 px-3 py-2">
            <summary className="cursor-pointer text-[12px] font-medium text-cyan-200">查看数据类别、保留周期与分享范围</summary>
            <div className="mt-2 space-y-2 text-[11px] leading-5 text-slate-400">
              <p><strong className="text-slate-200">WebView 页面记录：</strong>快速导航最多保留 5 个页面 ID，可在其快捷面板中独立清除；网络质量历史含时间、探测数量及延迟/抖动/丢包，最多 300 条且不超过 24 小时，探测仅在用户操作后向默认网关及 223.5.5.5、119.29.29.29 发送 ICMP，公网目标可见探测来源地址；磁盘趋势含盘符、容量、可用空间和采样时间，打开仪表盘时采样、保留 14 天；进程资源历史默认关闭，开启后每 5 分钟最多采样一次、每次最多 20 个程序名及其 CPU/内存汇总、保留 7 天，不保存 PID、路径或命令行；电池样本默认关闭，开启后保留 48 小时，含采样时间、电量和供电状态。</p>
              <p><strong className="text-slate-200">优化实验：</strong>最多保留 8 条汇总记录；待续实验草稿保存在当前会话存储并在 2 小时后过期。它是恢复状态，不包含在上方的历史清理操作中。</p>
              <p><strong className="text-slate-200">Roaming AppData：</strong>自动化配置会持续保存；屏幕亮度恢复快照只含原始/变暗百分比，变暗前写入、恢复后清除；成功变暗后的会话恢复及启动/退出恢复，仅在实时亮度匹配上次变暗目标时写回原值，外部改动不会被覆盖，读取/恢复失败会保留快照并提示重试；场景/驻留建议记录 30 天未使用后按源码规则淘汰；磁盘健康记录包含健康状态、温度、磨损和错误计数等 SMART 摘要，每盘最多 120 个样本，连续 30 天未见的磁盘会被清理；变更日志没有自动淘汰，启动恢复数据为恢复用途保留，计划备份保留至移除；识别的 AeroTune 日期日志在启动时清理 14 天以前的文件。相关内容可能含程序名、路径或系统恢复快照，AeroTune 不另行加密。</p>
              <p><strong className="text-slate-200">探测与导出：</strong>诊断报告先预览，设备标识和详细指标默认不含；分别选择后才加入，并仅保存至用户选择的位置，不会自动上传。配置导出仅含自动化设置、名单和规则。当前卸载数据保留/删除及安装后保留周期尚未通过一次性 Windows VM 验证。</p>
            </div>
          </details>
          <div className="mt-3 rounded-lg border border-white/8 bg-white/3 p-3">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0 flex-1">
                <div className="text-[12px] font-medium text-slate-200">清除所列本机观察历史</div>
                <p className="mt-1 text-[11px] leading-5 text-slate-400">
                  只对以下 5 个 WebView localStorage 历史键调用删除：{LOCAL_OBSERVATION_HISTORY_TARGETS.map((target) => target.label).join("、")}。不会清除设置、撤销日志/快照、启动或计划恢复备份、应用日志、快速导航记录或可恢复的实验会话草稿；这不是全部数据清除，也不承诺底层介质安全擦除。清除进程历史会同时把其采样开关恢复为默认关闭；继续使用相应功能后，部分历史可能重新生成。
                </p>
              </div>
              <Btn variant="danger" onClick={() => runLocalHistoryCleanup()}>
                <Trash2 size={13} /> 清除所列历史
              </Btn>
            </div>
            {localHistoryResults && localHistoryResults.some((result) => !result.ok) && (
              <div role="alert" className="mt-3 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-[11px] leading-5 text-amber-100">
                <span className="min-w-0 flex-1 break-words">
                  删除 API 未报告失败：{localHistoryResults.filter((result) => result.ok).map((result) => result.label).join("、") || "无"}。未完成：{localHistoryResults.filter((result) => !result.ok).map((result) => result.label).join("、")}；失败项可能仍保留。
                </span>
                <Btn variant="ghost" onClick={() => runLocalHistoryCleanup(true)}>重试失败项</Btn>
              </div>
            )}
            {localHistoryResults && localHistoryResults.length > 0 && localHistoryResults.every((result) => result.ok) && (
              <div role="status" className="mt-3 text-[11px] leading-5 text-emerald-200">
                已对 {localHistoryResults.length} 项发出本机 WebView 存储删除请求，且 API 未报告失败；不存在的键也安全忽略。此结果不等同于底层介质安全擦除。
              </div>
            )}
          </div>
        </Card>

        <Card id="settings-restore-point" focusableTarget title="系统还原点" icon={<Power size={15} className="text-emerald-300" />}>
          <div className="mb-3 flex items-center gap-2">
            <Btn onClick={createRestore} disabled={creating} className="shrink-0">
              {creating ? "创建中…" : "创建还原点"}
            </Btn>
            <div className="min-w-0 flex-1 text-[12px] text-slate-500">
              需要系统保护已开启（控制面板 → 系统保护）
            </div>
          </div>
          <div className="flex items-center justify-between border-t border-white/6 pt-3">
            <div className="pr-4 text-[13px] text-slate-400">
              重大调优（HAGS / 服务修改）前自动打点
            </div>
            {autoRestore === null ? <Chip tone={settingsStatus === "loading" ? "slate" : "amber"}>{settingsStatus === "loading" ? "读取中…" : "未读取"}</Chip> : <Toggle label="重大调优前自动创建还原点" on={autoRestore} onChange={toggleAutoRestore} disabled={settingsStatus === "loading"} />}
          </div>
          {shadowsStatus === "loading" && <div role="status" className="mt-3 text-xs text-slate-500">正在读取还原点存储…</div>}
          {shadowsStatus === "error" && (
            <div role="alert" className="mt-3 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
              <span className="min-w-0 flex-1 break-words">还原点存储读取失败。</span>
              <Btn variant="ghost" className="shrink-0" onClick={() => void loadShadows()}>重试</Btn>
            </div>
          )}
          {shadowsStatus === "ready" && shadows.length === 0 && <div role="status" className="mt-3 text-xs text-slate-500">Windows 未返回还原点存储数据。</div>}
          {shadowsStatus === "ready" && shadows.length > 0 && (
            <div className="mt-4 border-t border-white/6 pt-3">
              <div className="mb-2 text-[11px] text-slate-500">还原点存储占用（配额满了会静默丢弃旧还原点）</div>
              {shadows.map((sh) => (
                <div key={sh.drive} className="mb-2 flex items-center gap-2 rounded-lg border border-white/6 bg-white/3 px-3 py-2 text-[12px]">
                  <span className="w-10 font-medium text-slate-200">{sh.drive}</span>
                  <span className="flex-1 tabular-nums text-slate-400">
                    已用 {sh.used_gb} GB / 配额 {sh.max_gb > 0 ? sh.max_gb + " GB" : "未限制"}
                  </span>
                  <Btn variant="ghost" onClick={() => delOldest(sh.drive)}>
                    删最旧
                  </Btn>
                  <Btn variant="ghost" onClick={() => capShadow(sh.drive, 10)}>
                    配额 10GB
                  </Btn>
                </div>
              ))}
            </div>
          )}
        </Card>
      </div>

      <Card
        id="settings-change-journal"
        focusableTarget
        className="mt-5"
        title={`变更日志（${changes.length} 条记录，${changes.filter(c => c.revert.t !== "none").length} 条支持还原）`}
        icon={<History size={15} className="text-violet-300" />}
        right={
          <div className="flex flex-wrap gap-2">
            <Btn variant="ghost" onClick={() => exportJournal("csv")} disabled={journalExporting || filteredChanges.length === 0}>
              {journalExporting ? "导出中…" : "导出筛选 CSV"}
            </Btn>
            <Btn variant="ghost" onClick={() => exportJournal("json")} disabled={journalExporting || filteredChanges.length === 0}>
              导出筛选 JSON
            </Btn>
            <Btn variant="ghost" onClick={load}>
              刷新
            </Btn>
            <Btn variant="danger" onClick={revertAll} disabled={changesStatus !== "ready" || !changes.some(c => c.revert.t !== "none")}>
              <Undo2 size={14} /> 一键全部还原
            </Btn>
          </div>
        }
      >
        {changesStatus === "loading" && <div role="status" className="mb-3 text-xs text-slate-500">正在读取变更日志…</div>}
        {changesStatus === "error" && (
          <div role="alert" className="mb-3 flex flex-wrap items-start gap-3 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-xs text-amber-100">
            <span className="min-w-0 flex-1 break-words">{changes.length > 0
              ? `变更日志读取失败，当前仅显示上次成功快照；恢复操作已禁用。${changesError ? ` ${changesError}` : ""}`
              : `变更日志不可用，无法确认是否为空；请保留原文件并修复读取/校验问题。${changesError ? ` ${changesError}` : ""}`}</span>
            <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
            <details className="basis-full rounded-md border border-amber-200/15 bg-black/10 px-3 py-2">
              <summary className="cursor-pointer font-medium text-amber-100">安全恢复步骤（不会自动修改文件）</summary>
              <ol className="mt-2 list-decimal space-y-1 pl-5 text-[11px] leading-5 text-slate-300">
                <li>先点“重试”；如果读取恢复正常，无需手工处理。日志不可用时，所有撤销/恢复操作保持禁用。</li>
                <li>如果仍显示上次成功的变更列表，可先导出筛选 CSV/JSON 留存描述性清单；导出不含撤销快照，不能修复日志或用于还原。</li>
                <li>若错误仍指向日志不可读或格式/校验问题：退出 AeroTune 后复制并保留当前 <code>changes.json</code> 原件；常见位置为 <code>%APPDATA%\AeroTune\changes.json</code>，重定向配置以当前 Windows 用户的 Roaming AppData 为准。</li>
                <li>只有在持有已验证可信的备份时，才在应用关闭后恢复该备份。不要删除、清空、新建或手工编辑原日志；没有可信备份时保留原件并停止，不要尝试“一键全部还原”。</li>
                <li>重新打开应用后点“刷新”；仅当日志成功加载，再逐条核对目标并使用单条还原。当前没有自动隔离或原位修复流程。</li>
              </ol>
            </details>
          </div>
        )}
        {recoveryGroups.length > 0 && <div className="mb-4 rounded-lg border border-cyan-400/15 bg-cyan-400/4 p-3">
          <div className="mb-2 flex items-center gap-2 text-xs font-medium text-cyan-200"><Database size={13} />恢复胶囊 · 可整组还原</div>
          <div className="mb-3 text-[11px] leading-4 text-slate-500">场景模式按组保存原始快照；恢复前会核对当前值仍与 AeroTune 写入值一致。若已被其他操作改动，胶囊会停止并保留未恢复记录。</div>
          <div className="space-y-1.5">{recoveryGroups.slice(0, 8).map((group) => <div key={group.id} className="flex flex-wrap items-center gap-2 rounded-md border border-white/6 bg-black/10 px-3 py-2">
            <span className="min-w-0 flex-1 truncate text-xs text-slate-300">{group.label}</span>
            <span className="text-[10px] text-slate-500">{group.count} 条 · {fmtTime(group.ts)}</span>
            <Btn variant="ghost" className="px-2 py-1 text-[11px]" onClick={() => revertGroup(group.id, group.label, group.count)} disabled={changesStatus !== "ready"}><Undo2 size={12} />还原此组</Btn>
          </div>)}</div>
        </div>}
        {changes.length > 0 && <div className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-white/6 bg-black/10 p-2.5">
          <input
            type="search"
            value={journalQuery}
            onChange={(event) => setJournalQuery(event.target.value)}
            placeholder="搜索描述、分组或记录 ID…"
            aria-label="搜索变更日志"
            className="min-w-[180px] flex-1 rounded-md border border-white/10 bg-white/3 px-3 py-2 text-xs text-slate-200 outline-none placeholder:text-slate-600 focus:border-cyan-400/30"
          />
          <select aria-label="变更日志类别筛选" value={journalCategory} onChange={(event) => setJournalCategory(event.target.value as ChangeCategoryFilter)} className="rounded-md border border-white/10 bg-[#171923] px-2.5 py-2 text-xs text-slate-300 outline-none">
            <option value="all">全部类别</option><option value="registry">注册表</option><option value="power_plan">电源</option><option value="tcp">TCP</option><option value="dns">DNS</option><option value="other">其他</option>
          </select>
          <select aria-label="变更日志还原状态筛选" value={journalReversibility} onChange={(event) => setJournalReversibility(event.target.value as ChangeReversibilityFilter)} className="rounded-md border border-white/10 bg-[#171923] px-2.5 py-2 text-xs text-slate-300 outline-none">
            <option value="all">全部记录</option><option value="reversible">可还原</option><option value="info">仅信息记录</option>
          </select>
          <span aria-live="polite" className="text-[11px] text-slate-500">显示 {filteredChanges.length}/{changes.length}</span>
        </div>}
        {changesStatus === "ready" && changes.length === 0 ? (
          <div className="py-8 text-center text-sm text-slate-500">
            <History className="mx-auto mb-2 opacity-40" />
            暂无变更记录 — 你在场景模式 / 调优开关 / 网络页做的每一项修改都会出现在这里
          </div>
        ) : changes.length > 0 && filteredChanges.length === 0 ? (
          <div className="py-8 text-center text-sm text-slate-500">没有符合当前搜索和筛选条件的记录</div>
        ) : changes.length > 0 ? (
          <div className="max-h-80 space-y-1.5 overflow-y-auto pr-1">
            {filteredChanges.map((c) => (
              <div
                key={c.id}
                className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2.5"
              >
                <span className="shrink-0 text-[11px] tabular-nums text-slate-500">{fmtTime(c.ts)}</span>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px] text-slate-300">{c.desc}</div>
                  {c.revert.t !== "none" && <div className="truncate text-[10px] text-amber-200/75" title={getChangeRevertTarget(c)}>{getChangeRevertTarget(c)}</div>}
                </div>
                <Chip tone="slate">{getChangeCategory(c.revert.t)}</Chip>
                {c.revert.t === "none" ? <span className="text-[11px] text-amber-300">日志不可撤销 · 使用专用恢复（如有）</span> : <button
                  onClick={() => revertOne(c)}
                  disabled={changesStatus !== "ready"}
                  className="rounded-md p-1.5 text-slate-400 hover:bg-white/10 hover:text-cyan-300 disabled:cursor-not-allowed disabled:opacity-40"
                  title={`还原目标：${getChangeRevertTarget(c)}`}
                >
                  <Undo2 size={14} />
                </button>}
              </div>
            ))}
          </div>
        ) : null}
        {changes.length > 0 && (
          <button
            disabled={changesStatus !== "ready"}
            onClick={async () => {
              if (changesStatus !== "ready") return;
              if (!confirm(`清空将永久移除当前 ${changes.length} 条日志记录及其恢复快照，不还原任何变更。日志变化时操作将取消，请刷新后重新确认。确定继续？`)) return;
              try {
                await api.clearJournal(changes);
                toastOk("日志已清空（不还原任何变更）");
              } catch (e) { toastErr(e); } finally { void load(); }
            }}
            className="mt-3 inline-flex items-center gap-1 text-[11px] text-slate-600 hover:text-slate-400 disabled:cursor-not-allowed disabled:opacity-40"
          >
            <Trash2 size={11} /> 清空日志（保留当前状态，仅移除记录）
          </button>
        )}
      </Card>

      <Card className="mt-5 mb-5" title="Windows 快捷入口" icon={<Monitor size={15} className="text-cyan-300" />}>
        <div className="mb-3 text-[12px] text-slate-500">一键打开常用系统设置页面；仅启动 Windows 设置，不修改系统配置。</div>
        <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
          {[
            ["display", "显示", Monitor], ["sound", "声音", Volume2],
            ["network", "网络", Wifi], ["windows_update", "Windows 更新", RefreshCw],
            ["startup", "启动应用", Rocket], ["storage", "存储", HardDrive],
            ["apps", "已安装应用", AppWindow], ["bluetooth", "蓝牙", Bluetooth],
            ["night_light", "夜间模式", MoonStar],
          ].map(([page, label, Icon]) => {
            const pageId = page as string;
            const ShortcutIcon = Icon as typeof Monitor;
            return <button key={pageId} disabled={openingSettings !== null} onClick={async () => {
              if (openingSettingsRef.current) return;
              openingSettingsRef.current = true;
              setOpeningSettings(pageId);
              try { await api.openSettingsPage(pageId); }
              catch (e) { toastErr(e); }
              finally { openingSettingsRef.current = false; setOpeningSettings(null); }
            }}
              className="flex items-center gap-2 rounded-lg border border-white/8 bg-white/3 px-3 py-2.5 text-left text-[12px] text-slate-300 transition-colors hover:border-cyan-400/25 hover:bg-cyan-400/8 disabled:opacity-50">
              <ShortcutIcon size={14} className="shrink-0 text-cyan-300" />{openingSettings === pageId ? "正在打开…" : label as string}
            </button>;
          })}
        </div>
      </Card>

      <Card id="settings-winget-updates" focusableTarget className="mt-5" title="已安装软件更新观察（WinGet）" icon={<RefreshCw size={15} className="text-cyan-300" />}>
        <div className="space-y-3 text-[12px] text-slate-400">
          <p className="leading-5">仅在你主动确认后运行 WinGet 的只读查询。查询可能访问已配置的软件源并更新其缓存，WinGet 也可能按源行为发送查询或包标识；AeroTune 不上传或持久化结果。不接受源协议，不安装或升级软件。WinGet 的表格受系统语言与终端格式影响，AeroTune 只显示原始文本，不解析或声称更新数量完整。</p>
          <Btn variant="ghost" disabled={checkingWinget} onClick={() => void checkWingetUpdates()}>
            <RefreshCw size={13} className={checkingWinget ? "animate-spin" : ""} />
            {checkingWinget ? "正在查询（最多约 90 秒）…" : "手动检查软件更新"}
          </Btn>
          <div aria-live="polite" aria-busy={checkingWinget}>
            {checkingWinget && <p role="status">正在查询。若需要源协议同意、WinGet 不可用、网络失败或超时，本次会显示失败而不会报告“无更新”。</p>}
            {wingetError && <p role="alert" className="break-words text-amber-200">无法确认软件更新状态：{wingetError}</p>}
            {wingetResult && (
              <div className="space-y-2">
                <p role="status">WinGet 查询命令已成功完成 · {fmtTime(wingetResult.checked_at_unix)} · {wingetResult.duration_ms} ms。以下原始输出未经结构化解析，不代表 AeroTune 已验证所有应用或更新。</p>
                <pre aria-label="WinGet 原始查询输出" className="max-h-72 overflow-auto whitespace-pre-wrap break-all rounded-lg border border-white/8 bg-slate-950/70 p-3 font-mono text-[11px] leading-5 text-slate-300">{wingetResult.output}</pre>
              </div>
            )}
          </div>
        </div>
      </Card>

      <Card id="settings-security-updates" focusableTarget className="mt-5" title="安全更新" icon={<RefreshCw size={15} className="text-cyan-300" />}>
        <div className="flex flex-wrap items-center gap-3 text-[12px] text-slate-400" aria-live="polite">
          <span>AeroTune v{appVersion}</span>
          {updateReadinessError && <span role="alert" className="text-amber-200">更新状态读取失败：{updateReadinessError}</span>}
          {!updateReadiness && !updateReadinessError && <span role="status">正在读取更新配置…</span>}
          {updateReadiness && !updateReadiness.enabled && (
            <span>{updateReadiness.reason ?? "更新服务未启用。"} 当前版本不会自动安装未验证的程序。</span>
          )}
          {updateReadiness?.enabled && (
            <>
              <span>更新来自 GitHub Releases；下载后由 AeroTune 有界下载器校验 Tauri 签名、版本与 NSIS 压缩包。安装前需再次确认，Windows 安装器会请求 UAC；不做静默提权。</span>
              <Btn variant="ghost" disabled={checkingUpdate || installingUpdate} onClick={() => void checkForSignedUpdate()}>
                {checkingUpdate ? "正在检查…" : "检查更新"}
              </Btn>
              {availableUpdate && (
                <Btn variant="primary" disabled={installingUpdate} onClick={() => void installSignedUpdate()}>
                  {installingUpdate ? "正在下载并启动安装器…" : `安装 v${availableUpdate.available_version}`}
                </Btn>
              )}
            </>
          )}
        </div>
      </Card>

      <Card className="mt-5" title="关于" icon={<Info size={15} className="text-slate-300" />}>
        <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-[13px] text-slate-400">
          <span>
            <b className="text-slate-200">AeroTune</b> v{appVersion}
          </span>
          <span>引擎：Rust + Tauri 2 + WebView2</span>
          <span>前端：React 18 + Tailwind 4</span>
          {infoStatus === "loading" && <span role="status" className="text-slate-500">正在读取系统信息…</span>}
          {infoStatus === "error" && <span role="alert" className="text-amber-200">系统信息读取失败</span>}
          {infoStatus === "error" && <Btn variant="ghost" onClick={() => void loadInfo()}>重试</Btn>}
          {info && (
            <>
              <span>
                {info.os} {info.version}（{info.arch}）
              </span>
              <span className="max-w-96 truncate" title={info.exe}>
                安装位置：{info.exe}
              </span>
            </>
          )}
          <button
            disabled={!info}
            className="ml-auto inline-flex items-center gap-1 text-cyan-300/90 hover:underline disabled:cursor-not-allowed disabled:opacity-50"
            onClick={() => {
              if (info) void api.openPath(info.exe).catch(toastErr);
            }}
          >
            <FolderOpen size={13} /> 打开所在目录
          </button>
        </div>
      </Card>
    </div>
  );
}
