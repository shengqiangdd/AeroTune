import { ArrowRight, Eye } from "lucide-react";
import { Card, Chip } from "./ui";

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

export function ScenarioPreviewCard({ mode }: { mode: ScenarioMode }) {
  const preview = PREVIEWS[mode];
  return (
    <Card title="场景变更预览" icon={<Eye size={15} className="text-cyan-300" />} right={<Chip tone={preview.tone}>{preview.label}</Chip>}>
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
