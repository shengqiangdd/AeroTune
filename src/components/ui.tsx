// 共享 UI 原子组件：卡片 / 开关 / 环形仪表 / 走势图 / 进度条 / 按钮
import { ReactNode } from "react";

export function Card({
  title,
  icon,
  right,
  children,
  className = "",
  id,
  focusableTarget = false,
}: {
  title?: string;
  icon?: ReactNode;
  right?: ReactNode;
  children: ReactNode;
  className?: string;
  id?: string;
  focusableTarget?: boolean;
}) {
  return (
    <div id={id} tabIndex={focusableTarget ? -1 : undefined} role={focusableTarget ? "region" : undefined} aria-label={focusableTarget ? title : undefined} className={`glass p-5 ${focusableTarget ? "focus:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/60" : ""} ${className}`}>
      {(title || right) && (
        <div className="mb-4 flex items-center justify-between">
          <div className="flex items-center gap-2">
            {icon}
            {title && <h2 className="text-sm font-medium text-slate-300">{title}</h2>}
          </div>
          {right}
        </div>
      )}
      {children}
    </div>
  );
}

export function Toggle({
  on,
  onChange,
  disabled = false,
  label,
}: {
  on: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  label: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!on)}
      className={`relative h-6 w-11 shrink-0 rounded-full transition-colors duration-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70 focus-visible:ring-offset-2 focus-visible:ring-offset-slate-950 ${
        on ? "grad-btn" : "bg-slate-600/50"
      } ${disabled ? "cursor-not-allowed opacity-40" : "cursor-pointer"}`}
    >
      <span
        className={`absolute top-0.5 h-5 w-5 rounded-full bg-white shadow transition-all duration-200 ${
          on ? "left-[22px]" : "left-0.5"
        }`}
      />
    </button>
  );
}

export function Ring({
  value,
  label,
  sub,
  size = 132,
  color = "#22d3ee",
  color2 = "#8b5cf6",
}: {
  value: number | null;
  label: string;
  sub?: string;
  size?: number;
  color?: string;
  color2?: string;
}) {
  const r = size / 2 - 10;
  const c = 2 * Math.PI * r;
  const pct = value == null || !Number.isFinite(value) ? null : Math.max(0, Math.min(100, value));
  const id = `g-${label}-${size}`;
  return (
    <div className="flex flex-col items-center gap-1.5">
      <div className="relative" style={{ width: size, height: size }}>
        <svg width={size} height={size} className="-rotate-90">
          <defs>
            <linearGradient id={id} x1="0%" y1="0%" x2="100%" y2="100%">
              <stop offset="0%" stopColor={color} />
              <stop offset="100%" stopColor={color2} />
            </linearGradient>
          </defs>
          <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="rgba(255,255,255,0.07)" strokeWidth="9" />
          <circle
            cx={size / 2}
            cy={size / 2}
            r={r}
            fill="none"
            stroke={`url(#${id})`}
            strokeWidth="9"
            strokeLinecap="round"
            strokeDasharray={c}
            strokeDashoffset={c * (1 - (pct ?? 0) / 100)}
            style={{ transition: "stroke-dashoffset 0.6s ease" }}
          />
        </svg>
        <div className="absolute inset-0 flex flex-col items-center justify-center">
          <span className="text-xl font-semibold tabular-nums text-slate-100">{pct == null ? "—" : `${pct.toFixed(0)}%`}</span>
          {sub && <span className="max-w-[104px] text-center text-[11px] leading-tight text-slate-400">{sub}</span>}
        </div>
      </div>
      <span className="text-xs text-slate-400">{label}</span>
    </div>
  );
}

export function Spark({
  data,
  max,
  color = "#22d3ee",
  height = 64,
}: {
  data: number[];
  max?: number;
  color?: string;
  height?: number;
}) {
  const w = 100;
  const pts = data.slice(-60);
  const peak = Math.max(max ?? 0, ...pts, 1);
  const step = pts.length > 1 ? w / (pts.length - 1) : w;
  const path =
    pts.length > 1
      ? pts
          .map((v, i) => `${i === 0 ? "M" : "L"}${(i * step).toFixed(2)},${(height - (v / peak) * (height - 6) - 3).toFixed(2)}`)
          .join(" ")
      : "";
  return (
    <svg viewBox={`0 0 ${w} ${height}`} preserveAspectRatio="none" className="h-full w-full">
      <defs>
        <linearGradient id={`spark-${color.replace("#", "")}`} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={color} stopOpacity="0.35" />
          <stop offset="100%" stopColor={color} stopOpacity="0" />
        </linearGradient>
      </defs>
      {path && (
        <>
          <path d={`${path} L${w},${height} L0,${height} Z`} fill={`url(#spark-${color.replace("#", "")})`} />
          <path d={path} fill="none" stroke={color} strokeWidth="1.6" vectorEffect="non-scaling-stroke" />
        </>
      )}
    </svg>
  );
}

export function Bar({ value, color = "grad-btn" }: { value: number; color?: string }) {
  return (
    <div className="h-2 w-full overflow-hidden rounded-full bg-white/8">
      <div
        className={`h-full rounded-full ${color.includes("#") ? "" : color}`}
        style={{ width: `${Math.min(100, Math.max(0, value))}%`, background: color.includes("#") ? color : undefined, transition: "width 0.5s ease" }}
      />
    </div>
  );
}

export function Btn({
  children,
  onClick,
  variant = "primary",
  disabled = false,
  className = "",
  ariaLabel,
  title,
}: {
  children: ReactNode;
  onClick?: () => void;
  variant?: "primary" | "ghost" | "danger";
  disabled?: boolean;
  className?: string;
  ariaLabel?: string;
  title?: string;
}) {
  const base =
    "inline-flex items-center justify-center gap-1.5 rounded-lg px-3.5 py-2 text-sm font-medium transition-all active:scale-[0.97] disabled:cursor-not-allowed disabled:opacity-40";
  const style =
    variant === "primary"
      ? "grad-btn text-white shadow-lg shadow-cyan-500/10 hover:brightness-110"
      : variant === "danger"
        ? "bg-rose-500/15 text-rose-300 border border-rose-500/30 hover:bg-rose-500/25"
        : "bg-white/6 text-slate-200 border border-white/10 hover:bg-white/12";
  return (
    <button type="button" disabled={disabled} onClick={onClick} aria-label={ariaLabel} title={title} className={`${base} ${style} focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70 focus-visible:ring-offset-2 focus-visible:ring-offset-slate-950 ${className}`}>
      {children}
    </button>
  );
}

export function Chip({ children, tone = "slate", className = "", role }: { children: ReactNode; tone?: "slate" | "cyan" | "violet" | "green" | "amber" | "rose"; className?: string; role?: "status" }) {
  const tones: Record<string, string> = {
    slate: "bg-white/8 text-slate-300 border-white/12",
    cyan: "bg-cyan-400/12 text-cyan-300 border-cyan-400/25",
    violet: "bg-violet-400/12 text-violet-300 border-violet-400/25",
    green: "bg-emerald-400/12 text-emerald-300 border-emerald-400/25",
    amber: "bg-amber-400/12 text-amber-300 border-amber-400/25",
    rose: "bg-rose-400/12 text-rose-300 border-rose-400/25",
  };
  return (
    <span role={role} className={`inline-flex items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] ${tones[tone]} ${className}`}>
      {children}
    </span>
  );
}

export function PageHead({ title, desc, right }: { title: string; desc: string; right?: ReactNode }) {
  return (
    <div className="mb-5 flex min-w-0 flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
      <div className="min-w-0">
        <h1 className="text-xl font-semibold text-slate-100">{title}</h1>
        <p className="mt-1 break-words text-[13px] text-slate-400">{desc}</p>
      </div>
      {right && <div className="flex flex-wrap items-center justify-start gap-2 sm:justify-end">{right}</div>}
    </div>
  );
}

export function ModeBanner({
  active,
  name,
  onExit,
}: {
  active: boolean;
  name: string;
  onExit?: () => void;
}) {
  if (!active) return null;
  return (
    <div className="rise mb-4 flex items-center gap-3 rounded-xl border border-cyan-400/25 bg-cyan-400/8 px-4 py-3">
      <span className="glow h-2 w-2 rounded-full bg-cyan-300" />
      <span className="text-sm text-cyan-200">
        <b>{name}</b> 已开启
      </span>
      {onExit && (
        <button onClick={onExit} className="ml-auto text-xs text-cyan-300/80 underline-offset-2 hover:underline">
          退出模式
        </button>
      )}
    </div>
  );
}
