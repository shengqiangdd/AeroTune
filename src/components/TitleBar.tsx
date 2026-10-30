// 自绘标题栏：拖拽区 / 管理员徽标 / 最小化·最大化·关闭
import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ShieldAlert, ShieldCheck, Minus, Square, X, Copy } from "lucide-react";

export function TitleBar({ admin }: { admin: boolean }) {
  const [maxed, setMaxed] = useState(false);
  const win = getCurrentWindow();

  useEffect(() => {
    const un = win.onResized(async () => setMaxed(await win.isMaximized()));
    return () => {
      un.then((f) => f());
    };
  }, []);

  const btn =
    "flex h-full w-12 items-center justify-center text-slate-400 transition-colors hover:bg-white/10 hover:text-slate-200";
  return (
    <header
      data-tauri-drag-region
      className="flex h-10 shrink-0 items-center border-b border-white/6 bg-black/20 select-none"
    >
      <div data-tauri-drag-region className="flex items-center gap-2 pl-4">
        <img src="app-icon.png" alt="" className="h-4.5 w-4.5 rounded" onError={(e) => ((e.target as HTMLImageElement).style.display = "none")} />
        <span className="text-[13px] font-semibold tracking-wide text-slate-200">
          Aero<span className="grad-text">Tune</span>
        </span>
        {admin ? (
          <span aria-label="当前以管理员身份运行" className="ml-1 inline-flex items-center gap-1 rounded-full border border-emerald-400/25 bg-emerald-400/10 px-1.5 py-px text-[10px] text-emerald-300">
            <ShieldCheck size={10} /> 管理员
          </span>
        ) : (
          <span aria-label="当前以标准权限运行，部分系统级操作需要管理员权限" title="部分系统级操作需要管理员权限，可在设置页按需提权" className="ml-1 inline-flex items-center gap-1 rounded-full border border-amber-400/25 bg-amber-400/10 px-1.5 py-px text-[10px] text-amber-200">
            <ShieldAlert size={10} /> 标准权限
          </span>
        )}
      </div>
      <div className="ml-auto flex h-full">
        <button type="button" className={`${btn} focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-cyan-300`} onClick={() => void win.minimize()} title="最小化" aria-label="最小化窗口">
          <Minus size={15} />
        </button>
        <button type="button" className={`${btn} focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-cyan-300`} onClick={() => void (maxed ? win.unmaximize() : win.maximize())} title={maxed ? "还原窗口" : "最大化窗口"} aria-label={maxed ? "还原窗口" : "最大化窗口"}>
          {maxed ? <Copy size={12} className="-scale-x-100" /> : <Square size={12} />}
        </button>
        <button
          type="button"
          className={`${btn} hover:!bg-rose-500/80 hover:!text-white focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-rose-300`}
          onClick={() => void win.close()}
          title="关闭"
          aria-label="关闭窗口"
        >
          <X size={15} />
        </button>
      </div>
    </header>
  );
}
