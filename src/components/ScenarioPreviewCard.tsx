import { ArrowRight, Eye, RefreshCw } from "lucide-react";
import type { GameModePreview, ModePreviewStatus } from "../lib/api";
import { Btn, Card, Chip } from "./ui";

export type ScenarioMode = "game" | "power" | "dev";

const PREVIEWS: Record<ScenarioMode, { label: string; tone: "rose" | "green" | "cyan"; actions: string[]; excluded: string[] }> = {
  game: {
    label: "游戏模式",
    tone: "rose",
    actions: ["切换到可用的高性能电源计划（按系统计划解析结果）", "关闭 GameDVR 后台录制", "支持时将 EPP 设为 0"],
    excluded: ["不会自动结束进程或清空内存；HAGS 与 GPU 应用偏好需单独设置"],
  },
  power: {
    label: "省电模式",
    tone: "green",
    actions: ["切换到节能电源计划", "关闭 GameDVR 后台录制", "支持时将 EPP 设为 85"],
    excluded: ["不会更改显示亮度、网络适配器或后台应用权限"],
  },
  dev: {
    label: "编码模式",
    tone: "cyan",
    actions: ["切换到平衡电源计划", "开启前台响应优先（Win32PrioritySeparation）", "支持时将 EPP 设为 25"],
    excluded: ["不会自动整理内存；“立即整理”是独立的手动操作"],
  },
};

type ScenarioPreviewCardProps = {
  mode: ScenarioMode;
  livePreview?: GameModePreview | null;
  previewStatus?: "loading" | "ready" | "error";
  previewError?: string | null;
  onRefreshPreview?: () => void;
};

const LIVE_STATUS_LABEL: Record<ModePreviewStatus, string> = {
  will_change: "将修改",
  unchanged: "已符合",
  requires_creation: "可能创建计划",
  unsupported: "不支持，将跳过",
  unknown: "状态未知 / 需注意",
};

function liveStatusClass(status: ModePreviewStatus) {
  if (status === "will_change" || status === "requires_creation") return "text-amber-200";
  if (status === "unchanged") return "text-emerald-200";
  if (status === "unsupported") return "text-slate-400";
  return "text-amber-200";
}

export function ScenarioPreviewCard({ mode, livePreview = null, previewStatus, previewError, onRefreshPreview }: ScenarioPreviewCardProps) {
  const preview = PREVIEWS[mode];
  return (
    <Card title="场景变更预览" icon={<Eye size={15} className="text-cyan-300" />} right={<Chip tone={preview.tone}>{preview.label}</Chip>}>
      {mode === "game" && (
        <section aria-label="游戏模式本机状态预检" className="mb-3 rounded-lg border border-cyan-300/12 bg-cyan-300/4 p-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-xs font-medium text-cyan-100">本机状态预检（只读）</h3>
            <Btn variant="ghost" className="shrink-0" disabled={previewStatus === "loading"} onClick={onRefreshPreview}>
              <RefreshCw size={12} className={previewStatus === "loading" ? "animate-spin" : ""} />
              {previewStatus === "loading" ? "正在检查…" : "刷新预检"}
            </Btn>
          </div>
          {previewStatus === "loading" && <p role="status" className="mt-2 text-[11px] text-slate-400">正在读取电源计划、GameDVR 配置和目标计划 AC EPP；不会修改系统。</p>}
          {previewStatus === "error" && (
            <p role="alert" className="mt-2 break-words text-[11px] leading-5 text-amber-200">
              {previewError || "本机状态无法可靠读取；为避免基于未知状态应用，游戏模式暂不可用。"}
            </p>
          )}
          {previewStatus === "ready" && livePreview && (
            <>
              <p className="mt-2 text-[10px] leading-4 text-slate-400">
                快照于 {new Date(livePreview.observed_at_unix_ms).toLocaleTimeString()} 读取，有效 2 分钟；应用时会复核活动计划、GameDVR 注册表值和已存在目标计划的 AC EPP，不一致时拒绝继续。
              </p>
              {livePreview.requires_confirmation && (
                <p role="note" className="mt-2 break-words text-[11px] leading-5 text-amber-200">
                  未发现现成高性能计划；点击开启时会先明确确认，可能复制系统模板。预检本身不会创建计划。
                </p>
              )}
              <ul className="mt-2 space-y-2">
                {livePreview.steps.map((step) => (
                  <li key={step.id} className="rounded-md border border-white/6 bg-black/10 px-2.5 py-2">
                    <div className="flex flex-wrap items-start justify-between gap-x-3 gap-y-1">
                      <span className="text-[11px] font-medium text-slate-200">{step.label}</span>
                      <span className={`text-[10px] ${liveStatusClass(step.status)}`}>{LIVE_STATUS_LABEL[step.status]}</span>
                    </div>
                    <p className="mt-1 break-words text-[10px] leading-4 text-slate-400">当前：{step.current} → 目标：{step.target}</p>
                    {step.note && <p className="mt-1 break-words text-[10px] leading-4 text-slate-500">{step.note}</p>}
                  </li>
                ))}
              </ul>
            </>
          )}
        </section>
      )}
      {mode !== "game" && <p className="mb-3 text-[10px] leading-4 text-slate-500">以下为预期变更范围；此模式的本机当前值尚未由状态化预检读取。</p>}
      <div className="grid gap-3 md:grid-cols-2">
        <section>
          <h3 className="mb-2 text-xs font-medium text-slate-300">开启时尝试应用</h3>
          <ul className="space-y-1.5">{preview.actions.map((action) => <li key={action} className="flex gap-2 text-xs leading-5 text-slate-400"><ArrowRight size={12} className="mt-1 shrink-0 text-cyan-300" />{action}</li>)}</ul>
        </section>
        <section>
          <h3 className="mb-2 text-xs font-medium text-slate-300">范围说明</h3>
          <ul className="space-y-1.5">{preview.excluded.map((item) => <li key={item} className="text-xs leading-5 text-slate-400">{item}</li>)}</ul>
          <p className="mt-2 text-[10px] leading-4 text-slate-600">EPP 在不支持的平台会跳过；系统计划可用性和写入结果以应用后的提示及设置页恢复日志为准。</p>
        </section>
      </div>
    </Card>
  );
}

export default ScenarioPreviewCard;
