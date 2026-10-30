import { Cpu } from "lucide-react";
import type { Stats } from "../lib/api";
import { summarizeCpuTopology } from "../lib/cpuTopology.js";
import { Card, Chip } from "./ui";

export function CpuTopologyCard({ stats, physicalCores }: { stats: Stats | null; physicalCores?: number | null }) {
  const { logical, mean, peak } = summarizeCpuTopology(stats?.per_core);

  return (
    <Card
      title="CPU 逻辑处理器拓扑"
      icon={<Cpu size={15} className="text-cyan-300" />}
      right={<Chip tone={logical.length ? "cyan" : "amber"}>{logical.length ? `${logical.length} 个逻辑处理器` : "数据未提供"}</Chip>}
    >
      <div className="mb-3 flex flex-wrap items-center gap-x-4 gap-y-1 text-[11px] text-slate-400">
        <span>物理核心：{physicalCores != null && physicalCores > 0 ? physicalCores : "未提供"}</span>
        <span>逻辑处理器平均：{mean == null ? "—" : `${mean.toFixed(1)}%`}</span>
        <span>峰值：{peak == null ? "—" : `${peak.toFixed(1)}%`}</span>
      </div>
      {logical.length > 0 ? (
        <div className="grid grid-cols-4 gap-2 sm:grid-cols-6 md:grid-cols-8 xl:grid-cols-12" aria-label="各逻辑处理器当前使用率">
          {logical.map((usage, index) => {
            const color = usage >= 85 ? "bg-rose-400" : usage >= 60 ? "bg-amber-300" : "bg-cyan-300";
            return (
              <div key={index} className="rounded-md border border-white/6 bg-black/10 px-2 py-1.5" title={`逻辑处理器 ${index}: ${usage.toFixed(1)}%`}>
                <div className="mb-1 flex items-center justify-between gap-1 text-[9px] text-slate-500"><span>CPU {index}</span><span className="tabular-nums">{usage.toFixed(0)}%</span></div>
                <div className="h-1.5 overflow-hidden rounded-full bg-white/8"><div className={`h-full rounded-full ${color}`} style={{ width: `${usage}%` }} /></div>
              </div>
            );
          })}
        </div>
      ) : (
        <div className="rounded-md border border-amber-300/10 bg-amber-300/4 px-3 py-3 text-xs text-slate-400">等待 sysinfo 提供逐逻辑处理器使用率。</div>
      )}
      <div className="mt-3 text-[10px] leading-4 text-slate-600">只读可视化 Windows 当前返回的逻辑处理器索引与负载，不推断核心簇、P/E 核类型或处理器组映射。</div>
    </Card>
  );
}

export default CpuTopologyCard;
