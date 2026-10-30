// 极简全局 Toast
import { useEffect, useState } from "react";
import { CheckCircle2, AlertTriangle, Info, X } from "lucide-react";

export type ToastKind = "ok" | "warn" | "info" | "bad";
export interface ToastItem {
  id: number;
  kind: ToastKind;
  text: string;
}

let nextId = 1;
const listeners = new Set<(items: ToastItem[]) => void>();
let items: ToastItem[] = [];

function emit() {
  for (const l of listeners) l([...items]);
}

export function toast(kind: ToastKind, text: string) {
  const id = nextId++;
  items = [...items, { id, kind, text }];
  emit();
  setTimeout(() => {
    items = items.filter((t) => t.id !== id);
    emit();
  }, 4200);
}

export function toastOk(text: string) {
  toast("ok", text);
}
export function toastErr(e: unknown) {
  toast("bad", typeof e === "string" ? e : String(e));
}

const ICONS: Record<ToastKind, React.ReactNode> = {
  ok: <CheckCircle2 size={17} className="text-emerald-400" />,
  warn: <AlertTriangle size={17} className="text-amber-400" />,
  info: <Info size={17} className="text-cyan-400" />,
  bad: <AlertTriangle size={17} className="text-rose-400" />,
};

const BAR: Record<ToastKind, string> = {
  ok: "bg-emerald-400",
  warn: "bg-amber-400",
  info: "bg-cyan-400",
  bad: "bg-rose-400",
};

export function Toaster() {
  const [list, setList] = useState<ToastItem[]>([]);
  useEffect(() => {
    listeners.add(setList);
    return () => {
      listeners.delete(setList);
    };
  }, []);
  return (
    <div aria-live="polite" aria-atomic="false" className="fixed bottom-5 right-5 z-50 flex w-[min(90vw,420px)] flex-col gap-2">
      {list.map((t) => (
        <div key={t.id} role={t.kind === "bad" ? "alert" : "status"} className="glass rise relative flex items-start gap-2.5 overflow-hidden p-3.5 shadow-2xl">
          <div className={`absolute left-0 top-0 h-full w-1 ${BAR[t.kind]}`} />
          <span className="mt-0.5 shrink-0">{ICONS[t.kind]}</span>
          <span className="flex-1 text-sm leading-5 text-slate-200">{t.text}</span>
          <button
            type="button"
            aria-label="关闭通知"
            className="shrink-0 text-slate-500 hover:text-slate-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300"
            onClick={() => {
              items = items.filter((x) => x.id !== t.id);
              emit();
            }}
          >
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
