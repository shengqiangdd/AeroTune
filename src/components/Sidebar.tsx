// 侧边导航
import {
  Gauge,
  Gamepad2,
  BatteryCharging,
  Code2,
  BrainCircuit,
  Cpu,
  Rocket,
  Trash2,
  Wifi,
  Server,
  Settings,
  Search,
} from "lucide-react";
import { Stats } from "../lib/api";
import { validCapacityPair } from "../lib/resourceEvidence";

export type PageId =
  | "dashboard"
  | "game"
  | "power"
  | "dev"
  | "auto"
  | "processes"
  | "startup"
  | "services"
  | "cleaner"
  | "network"
  | "settings";

export const NAV: { id: PageId; label: string; icon: React.ReactNode; group: string }[] = [
  { id: "dashboard", label: "仪表盘", icon: <Gauge size={17} />, group: "总览" },
  { id: "game", label: "游戏模式", icon: <Gamepad2 size={17} />, group: "场景" },
  { id: "power", label: "省电续航", icon: <BatteryCharging size={17} />, group: "场景" },
  { id: "dev", label: "编码模式", icon: <Code2 size={17} />, group: "场景" },
  { id: "auto", label: "智能引擎", icon: <BrainCircuit size={17} />, group: "场景" },
  { id: "processes", label: "进程管理", icon: <Cpu size={17} />, group: "系统" },
  { id: "startup", label: "开机全景", icon: <Rocket size={17} />, group: "系统" },
  { id: "services", label: "服务管理", icon: <Server size={17} />, group: "系统" },
  { id: "cleaner", label: "空间清理", icon: <Trash2 size={17} />, group: "系统" },
  { id: "network", label: "网络优化", icon: <Wifi size={17} />, group: "系统" },
  { id: "settings", label: "设置", icon: <Settings size={17} />, group: "系统" },
];

export function Sidebar({
  page,
  onNav,
  stats,
  onQuickOpen,
  appVersion,
}: {
  page: PageId;
  onNav: (p: PageId) => void;
  stats: Stats | null;
  onQuickOpen: () => void;
  appVersion?: string;
}) {
  const groups = ["总览", "场景", "系统"];
  const memoryValid = stats && validCapacityPair(stats.mem_used_gb, stats.mem_total_gb);
  return (
    <aside className="flex min-h-0 w-52 shrink-0 flex-col gap-1 border-r border-white/6 bg-black/15 px-3 py-4 select-none">
      <nav aria-label="主导航" className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
        <button onClick={onQuickOpen} className="mb-3 flex items-center gap-2 rounded-lg border border-white/8 bg-white/4 px-3 py-2 text-left text-[12px] text-slate-400 transition-colors hover:border-cyan-400/25 hover:bg-white/8 hover:text-slate-200" title="快速跳转页面（Ctrl+K）">
          <Search size={13} /> 快速跳转 <kbd className="ml-auto rounded border border-white/10 px-1 py-0.5 text-[9px]">Ctrl K</kbd>
        </button>
        {groups.map((g) => (
          <div key={g} className="mb-2">
            <div className="mb-1.5 px-3 text-[10px] font-medium uppercase tracking-widest text-slate-500">
              {g}
            </div>
            <div className="flex flex-col gap-0.5">
              {NAV.filter((n) => n.group === g).map((n) => {
                const active = n.id === page;
                return (
                  <button
                    key={n.id}
                    data-page={n.id}
                    aria-current={active ? "page" : undefined}
                    onClick={() => onNav(n.id)}
                    className={`relative flex items-center gap-2.5 rounded-lg px-3 py-2 text-[13px] transition-all ${
                      active
                        ? "bg-white/8 font-medium text-white"
                        : "text-slate-400 hover:bg-white/5 hover:text-slate-200"
                    }`}
                  >
                    {active && (
                      <span className="absolute left-0 top-1/2 h-4.5 w-[3px] -translate-y-1/2 rounded-full grad-btn" />
                    )}
                    <span className={active ? "text-cyan-300" : ""}>{n.icon}</span>
                    {n.label}
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </nav>
      <div className="mt-auto px-2">
        <div className="glass p-3 text-[11px] text-slate-400" aria-label="实时内存系统已用量与总量">
          <div className="mb-1 flex items-center justify-between">
            <span className="text-slate-300">已用 / 总量</span>
            <span className="tabular-nums">
              {memoryValid && stats ? `${stats.mem_used_gb.toFixed(1)} / ${stats.mem_total_gb.toFixed(0)} GB` : stats ? "数据不可用" : "-"}
            </span>
          </div>
          <div className="h-1.5 overflow-hidden rounded-full bg-white/8">
            <div
              className="h-full rounded-full grad-btn transition-all duration-500"
              style={{
                width: memoryValid && stats ? `${(stats.mem_used_gb / stats.mem_total_gb) * 100}%` : "0%",
              }}
            />
          </div>
          <div className="mt-2 text-[10px] leading-4 text-slate-500">
            AeroTune v{appVersion} · Rust + Tauri 2
          </div>
        </div>
      </div>
    </aside>
  );
}
