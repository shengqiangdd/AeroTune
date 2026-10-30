// 网络优化：网卡 / DNS 预设 / TCP 自适应调节 / 刷新 DNS / 线路质量探测 / WiFi 配置
import { useCallback, useEffect, useRef, useState } from "react";
import { Wifi, RefreshCw, Globe, Network as NetworkIcon, Eraser, SignalHigh, Zap } from "lucide-react";
import { api, Adapter, DnsEntry, NetQuality, WifiProfile, NicPower, TcpLatency } from "../lib/api";
import { Btn, PageHead, Card, Chip, Toggle } from "../components/ui";
import { toastOk, toastErr, toast } from "../lib/toast";
import { NetworkQualityHistory } from "../components/NetworkQualityHistory";

const DNS_PRESETS: { name: string; primary: string; secondary: string; tone: "cyan" | "violet" | "green" | "amber" }[] = [
  { name: "阿里 DNS", primary: "223.5.5.5", secondary: "223.6.6.6", tone: "cyan" },
  { name: "腾讯 DNSPod", primary: "119.29.29.29", secondary: "182.254.116.116", tone: "violet" },
  { name: "Google", primary: "8.8.8.8", secondary: "8.8.4.4", tone: "green" },
  { name: "Cloudflare", primary: "1.1.1.1", secondary: "1.0.0.1", tone: "amber" },
];

function isIpv4Address(value: string): boolean {
  const parts = value.trim().split(".");
  return parts.length === 4 && parts.every((part) => /^(0|[1-9]\d{0,2})$/.test(part) && Number(part) <= 255);
}

const AUTO_LEVELS = ["disabled", "highlyrestricted", "restricted", "normal", "experimental"];
const LEVEL_CN: Record<string, string> = {
  disabled: "关闭",
  highlyrestricted: "高度受限",
  restricted: "受限",
  normal: "正常（推荐）",
  experimental: "实验性",
  unknown: "未知",
};

export function Network() {
  const [adapters, setAdapters] = useState<Adapter[]>([]);
  const [adapterError, setAdapterError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [dns, setDns] = useState<DnsEntry[]>([]);
  const [level, setLevel] = useState("unknown");
  const [tcpBusy, setTcpBusy] = useState(false);
  const [sel, setSel] = useState<string | null>(null);
  const [custom, setCustom] = useState("");
  const [dnsAction, setDnsAction] = useState<"apply" | "reset" | "flush" | null>(null);
  const customDnsValid = isIpv4Address(custom);
  const [quality, setQuality] = useState<NetQuality | null>(null);
  const [qualityError, setQualityError] = useState<string | null>(null);
  const [pinging, setPinging] = useState(false);
  const [wifi, setWifi] = useState<WifiProfile[] | null>(null);
  const [wifiLoading, setWifiLoading] = useState(false);
  const [wifiError, setWifiError] = useState<string | null>(null);
  const pingPending = useRef(false);
  const active = useRef(false);
  const loadPending = useRef(false);
  const wifiPending = useRef(false);
  const dnsActionPending = useRef(false);
  const tcpPending = useRef(false);

  useEffect(() => {
    active.current = true;
    return () => { active.current = false; };
  }, []);

  const loadWifi = useCallback(async () => {
    if (wifiPending.current) return;
    wifiPending.current = true;
    if (active.current) {
      setWifiLoading(true);
      setWifiError(null);
    }
    try {
      const profiles = await api.listWifiProfiles();
      if (active.current) setWifi(profiles);
    } catch (error) {
      if (active.current) setWifiError(String(error));
    } finally {
      wifiPending.current = false;
      if (active.current) setWifiLoading(false);
    }
  }, []);

  const load = useCallback(async () => {
    if (loadPending.current) {
      toast("info", "网络状态正在更新，请稍后重试");
      return;
    }
    loadPending.current = true;
    if (active.current) setLoading(true);
    try {
    const [adapterResult, dnsResult, tcpResult] = await Promise.allSettled([
      api.listAdapters(),
      api.getDns(),
      api.getTcpAutotuning(),
    ]);

    if (adapterResult.status === "fulfilled") {
      const a = adapterResult.value;
      if (active.current) {
        setAdapters(a);
        setAdapterError(null);
        setSel((current) => current && a.some((x) => x.name === current)
          ? current
          : a.find((x) => x.status === "Up")?.name ?? a[0]?.name ?? null);
      }
    } else {
      const message = String(adapterResult.reason);
      if (active.current) {
        setAdapterError(message);
        setSel(null);
        toastErr(adapterResult.reason);
      }
    }
    if (dnsResult.status === "fulfilled") { if (active.current) setDns(dnsResult.value); }
    else if (active.current) toastErr(dnsResult.reason);
    if (tcpResult.status === "fulfilled") { if (active.current) setLevel(tcpResult.value); }
    else {
      if (active.current) {
        setLevel("unknown");
        toastErr(tcpResult.reason);
      }
    }
    } finally {
      loadPending.current = false;
      if (active.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const runDnsAction = async (
    kind: "apply" | "reset" | "flush",
    action: () => Promise<void>,
    successMessage: string,
    reloadAfter = false,
  ) => {
    if (dnsActionPending.current || tcpPending.current) return;
    if (loadPending.current) {
      toast("info", "网络状态正在更新，请稍后再操作");
      return;
    }
    dnsActionPending.current = true;
    if (active.current) setDnsAction(kind);
    try {
      await action();
      if (active.current) toastOk(successMessage);
      if (reloadAfter && active.current) await load();
    } catch (e) {
      if (active.current) toastErr(e);
    } finally {
      dnsActionPending.current = false;
      if (active.current) setDnsAction(null);
    }
  };

  const applyDns = (primary: string, secondary: string) => {
    const iface = sel;
    if (!iface) return;
    return runDnsAction(
      "apply",
      () => api.setDns(iface, primary, secondary),
      `「${iface}」DNS → ${primary}${secondary ? " / " + secondary : ""}（可在设置页撤销）`,
      true,
    );
  };

  const resetDns = () => {
    const iface = sel;
    if (!iface) return;
    return runDnsAction(
      "reset",
      () => api.resetDns(iface),
      `「${iface}」DNS 已恢复自动获取（可在设置页撤销）`,
      true,
    );
  };

  const flush = () => runDnsAction("flush", () => api.flushDns(), "本机 DNS 缓存已刷新");

  const setAuto = async (lv: string) => {
    if (tcpPending.current || dnsActionPending.current || loadPending.current || level === "unknown") return;
    tcpPending.current = true;
    if (active.current) setTcpBusy(true);
    try {
      await api.setTcpAutotuning(lv);
      if (active.current) {
        setLevel(lv);
        toastOk(`TCP 接收窗口自动调节 → ${LEVEL_CN[lv] ?? lv}`);
      }
    } catch (e) {
      if (active.current) toastErr(e);
    } finally {
      tcpPending.current = false;
      if (active.current) setTcpBusy(false);
    }
  };

  const dnsOf = (name: string) => dns.find((d) => d.iface === name);

  const probe = async () => {
    if (pingPending.current) return;
    pingPending.current = true;
    setPinging(true);
    setQuality(null);
    setQualityError(null);
    try {
      const result = await api.pingQuality();
      if (active.current) { setQuality(result); setQualityError(null); setPinging(false); }
    } catch (e) {
      if (active.current) {
        setQualityError(String(e));
        toastErr(e);
        setPinging(false);
      }
    } finally {
      pingPending.current = false;
      if (active.current) setPinging(false);
    }
  };

  const statRow = (label: string, s: { ok: number; avg: number | null; sd: number | null; loss_pct: number } | null) => (
    <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1 rounded-lg border border-white/8 bg-white/3 px-3.5 py-2.5 text-[13px]">
      <span className="text-slate-400">{label}</span>
      {s == null ? (
        <span className="text-slate-600">—</span>
      ) : s.ok === 0 ? (
        <Chip tone="rose">不可达（丢包 100%）</Chip>
      ) : (
        <span className="flex items-center gap-3 tabular-nums">
          <span className="text-slate-200">
            延迟 <b className={s.avg != null && s.avg < 20 ? "text-emerald-300" : s.avg != null && s.avg < 60 ? "text-amber-300" : "text-rose-300"}>{s.avg?.toFixed(1) ?? "?"}ms</b>
          </span>
          <span className="text-slate-400">
            抖动 <b className={s.sd != null && s.sd < 3 ? "text-emerald-300" : "text-amber-300"}>{s.sd?.toFixed(1) ?? "?"}ms</b>
          </span>
          <span className="text-slate-400">
            丢包 <b className={s.loss_pct === 0 ? "text-emerald-300" : "text-rose-300"}>{s.loss_pct.toFixed(0)}%</b>
          </span>
        </span>
      )}
    </div>
  );

  return (
    <div>
      <PageHead
        title="网络优化"
        desc="网卡状态、DNS 一键切换、TCP 接收窗口调节，降低延迟提升解析速度"
        right={
          <Btn variant="ghost" onClick={load} disabled={loading || dnsAction !== null || tcpBusy}>
            <RefreshCw size={14} className={loading ? "animate-spin" : ""} />
            {loading ? "刷新中…" : dnsAction || tcpBusy ? "网络操作中…" : "刷新"}
          </Btn>
        }
      />

      <div className="mb-5 grid grid-cols-1 gap-4 xl:grid-cols-3">
        <Card className="min-w-0 xl:col-span-2" title="网卡" icon={<NetworkIcon size={15} className="text-cyan-300" />}>
          <div className="flex flex-col gap-1.5">
            {loading && adapters.length === 0 && <div className="py-6 text-center text-xs text-slate-500">正在读取网卡…</div>}
            {adapters.map((a) => (
              <button
                key={a.name}
                type="button"
                disabled={loading || adapterError !== null || dnsAction !== null || tcpBusy}
                onClick={() => setSel(a.name)}
                className={`flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1.5 rounded-lg border px-3 py-2.5 text-left transition-all disabled:cursor-not-allowed disabled:opacity-60 sm:flex-nowrap sm:px-4 ${
                  sel === a.name ? "border-cyan-400/35 bg-cyan-400/8" : "border-white/8 bg-white/3 hover:bg-white/7"
                }`}
              >
                <Wifi size={15} className={`shrink-0 ${a.status === "Up" ? "text-emerald-300" : "text-slate-500"}`} />
                <div className="min-w-0 flex-1 basis-[calc(100%-2rem)] sm:basis-0">
                  <div className="break-words text-[13px] font-medium text-slate-200">
                    {a.name}
                    {a.desc && <span className="ml-2 break-words text-[11px] font-normal text-slate-500">{a.desc}</span>}
                  </div>
                  {dnsOf(a.name) && (
                    <div className="break-all text-[11px] text-slate-500">DNS: {dnsOf(a.name)!.servers.join(" / ")}</div>
                  )}
                </div>
                <Chip className="shrink-0" tone={a.status === "Up" ? "green" : "slate"}>{a.status === "Up" ? "已连接" : a.status}</Chip>
                <span className="w-auto shrink-0 text-right text-[11px] tabular-nums text-slate-400 sm:w-20">{a.speed}</span>
              </button>
            ))}
            {adapterError && (
              <div role="alert" className="mt-2 flex flex-wrap items-start justify-between gap-3 rounded-lg border border-rose-300/20 bg-rose-300/5 px-3 py-2">
                <span className="min-w-0 flex-1 break-words text-xs text-rose-300">网卡读取失败；当前列表仅供查看，DNS 服务器选择与设置已暂停：{adapterError}</span>
                <Btn variant="ghost" className="shrink-0" disabled={loading} onClick={() => void load()}>
                  <RefreshCw size={12} className={loading ? "animate-spin" : ""} /> {loading ? "重试中…" : "重试"}
                </Btn>
              </div>
            )}
            {!loading && !adapterError && adapters.length === 0 && <div className="py-6 text-center text-xs text-slate-500">未发现可显示的网卡</div>}
          </div>
        </Card>

        <Card title="TCP 自适应调节" icon={<Globe size={15} className="text-violet-300" />}>
          <div role="status" aria-live="polite" aria-atomic="true" className="mb-2 text-[12px] text-slate-400">
            当前：<b className="text-violet-300">{LEVEL_CN[level] ?? level}</b>
            {loading ? " · 正在读取状态…" : tcpBusy ? " · 正在应用…" : level === "unknown" ? " · 当前级别未知，刷新确认后再修改。" : ""}
          </div>
          <div className="flex flex-col gap-1">
            {AUTO_LEVELS.map((lv) => (
              <button
                key={lv}
                type="button"
                disabled={loading || level === "unknown" || tcpBusy || dnsAction !== null}
                onClick={() => setAuto(lv)}
                className={`rounded-lg border px-3 py-2 text-left text-[13px] transition-all disabled:cursor-not-allowed disabled:opacity-50 ${
                  level === lv
                    ? "border-violet-400/40 bg-violet-400/10 text-violet-200"
                    : "border-white/8 bg-white/3 text-slate-300 hover:bg-white/8"
                }`}
              >
                {LEVEL_CN[lv]}
              </button>
            ))}
          </div>
          <div className="mt-3 text-[11px] text-slate-600">修改需管理员权限，已记录可还原</div>
        </Card>
      </div>

      <Card title={`DNS 设置 — ${sel ?? "请先选择网卡"}`} icon={<Globe size={15} className="text-cyan-300" />}>
        {dnsAction && (
          <div role="status" aria-live="polite" aria-atomic="true" className="mb-3 text-[11px] text-cyan-300">
            {dnsAction === "apply" ? "正在应用 DNS 配置…" : dnsAction === "reset" ? "正在恢复自动获取 DNS…" : "正在刷新本机 DNS 缓存…"}
          </div>
        )}
        <div className="mb-4 flex flex-wrap gap-2">
          {DNS_PRESETS.map((p) => (
            <button
              key={p.name}
              disabled={!sel || loading || dnsAction !== null || tcpBusy}
              onClick={() => applyDns(p.primary, p.secondary)}
              className="rounded-lg border border-white/10 bg-white/5 px-3.5 py-2 text-[13px] text-slate-200 transition-all hover:bg-white/12 disabled:opacity-40"
            >
              {p.name}
              <span className="ml-1.5 text-[11px] text-slate-500">{p.primary}</span>
            </button>
          ))}
        </div>
        <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap sm:items-center">
          <input
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            aria-label="自定义 IPv4 DNS 服务器地址"
            aria-invalid={custom.trim().length > 0 && !customDnsValid}
            inputMode="decimal"
            maxLength={15}
            placeholder="自定义 IPv4 DNS，如 114.114.114.114"
            className="w-full min-w-0 rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-slate-200 outline-none placeholder:text-slate-600 focus:border-cyan-400/40 sm:w-64"
          />
          <Btn disabled={!sel || !customDnsValid || loading || dnsAction !== null || tcpBusy} onClick={() => applyDns(custom.trim(), "")}>
            应用自定义 DNS
          </Btn>
          <Btn variant="ghost" disabled={!sel || loading || dnsAction !== null || tcpBusy} onClick={resetDns}>
            {dnsAction === "reset" ? "恢复中…" : "恢复自动获取"}
          </Btn>
          <div className="sm:ml-auto">
            <Btn variant="ghost" disabled={loading || dnsAction !== null || tcpBusy} onClick={flush}>
              <Eraser size={14} /> {dnsAction === "flush" ? "刷新中…" : "刷新本机 DNS 缓存"}
            </Btn>
          </div>
        </div>
        {custom.trim() && !customDnsValid && (
          <div role="alert" className="mt-2 text-[11px] text-rose-300">请输入有效 IPv4 地址（四段 0–255）。</div>
        )}
        <div className="mt-3 text-[11px] text-slate-500">
          DNS 变更会记录在「设置」页，可在配置未被外部修改时精确还原；“恢复自动获取”会同时重置 IPv4 与 IPv6 DNS。
        </div>
      </Card>

      <Card
        className="mt-5"
        title="线路质量探测"
        icon={<SignalHigh size={15} className="text-emerald-300" />}
        right={
          <Btn variant="ghost" onClick={probe} disabled={pinging}>
            {pinging ? "探测中（约 5 秒）…" : "开始探测"}
          </Btn>
        }
      >
        {quality == null && !pinging && !qualityError && (
          <div className="py-3 text-[13px] text-slate-500">
            对网关与公共 DNS 各发 4 次 ICMP 探测，统计平均延迟（RTT）、抖动（标准差）与丢包率，
            用于定位"卡"到底出在本机、运营商出口还是解析环节。
          </div>
        )}
        {pinging && <div className="h-24 animate-pulse rounded-lg bg-white/4" />}
        {qualityError && !pinging && (
          <div role="alert" className="rounded-lg border border-amber-400/20 bg-amber-400/5 px-3.5 py-3 text-xs leading-5 text-amber-200">
            探测未完成，未形成有效的丢包样本：{qualityError}。这不等同于网络丢包，可重试探测。
          </div>
        )}
        {quality && (() => {
          const summary = quality.summary;
          return <div>
            <div className="mb-3 flex flex-wrap items-center gap-2 rounded-lg border border-white/8 bg-white/3 px-3.5 py-3">
              <Chip tone={summary.tone}>{summary.label}</Chip>
              <span className="text-xs leading-5 text-slate-400">{summary.detail}</span>
            </div>
            <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
              {statRow("网关（默认路由下一跳）", quality.gateway)}
              {statRow("阿里 DNS 223.5.5.5", quality.dns)}
              {quality.second_dns && statRow("腾讯 DNS 119.29.29.29", quality.second_dns)}
            </div>
          </div>;
        })()}
      </Card>
      <div className="mt-5"><NetworkQualityHistory /></div>
      <Card
        className="mt-5"
        title="已保存的 Wi-Fi 网络"
        icon={<Wifi size={15} className="text-amber-300" />}
        right={
          <Btn variant="ghost" onClick={loadWifi} disabled={wifiLoading}>
            {wifiLoading ? "读取中…" : wifi === null ? "读取网络名称" : "重新读取"}
          </Btn>
        }
      >
        <div role="note" className="mb-3 rounded-lg border border-cyan-400/15 bg-cyan-400/5 px-3.5 py-3 text-xs leading-5 text-slate-300">
          仅在点击读取后显示本机已保存网络的名称与安全类型。为保护网络凭据，AeroTune 不读取、不显示或上传 Wi-Fi 密码。
        </div>
        {wifiError && <div role="alert" className="mb-3 rounded-lg border border-rose-400/20 bg-rose-400/5 px-3.5 py-3 text-xs leading-5 text-rose-200">读取未完成：{wifiError}</div>}
        {wifiLoading && <div className="py-4 text-center text-xs text-slate-500">正在读取网络名称…</div>}
        {wifi === null && !wifiLoading && !wifiError && (
          <div className="text-xs text-slate-500">尚未读取网络列表。</div>
        )}
        {wifi !== null && !wifiLoading && wifi.length === 0 && (
          <div className="text-xs text-slate-500">未读取到 WLAN 配置（无网卡或未保存过网络）</div>
        )}
        {wifi !== null && wifi.length > 0 && (
          <div className="space-y-1.5">
            {wifi.map((w) => (
              <div key={w.ssid} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2 text-[13px] sm:flex-nowrap">
                <Wifi size={14} className="shrink-0 text-slate-400" />
                <span className="min-w-0 flex-1 basis-[calc(100%-2rem)] truncate text-slate-200 sm:basis-0">{w.ssid}</span>
                <span className="max-w-full break-all text-[11px] text-slate-500">{w.auth || "认证信息不可用"}</span>
                <span className="max-w-full break-all text-right text-[11px] text-slate-400">{w.cipher || "加密信息不可用"}</span>
              </div>
            ))}
          </div>
        )}
      </Card>

      <div className="mt-5 flex items-center gap-2 text-sm font-medium text-slate-300">
        <Zap size={15} className="text-amber-300" /> 延迟优化二件套
      </div>
      <div className="mt-3">
        <NicLatencyCards />
      </div>
    </div>
  );
}

// ---------- 网卡节能 / TCP 延迟（游戏二件套） ----------

function NicLatencyCards() {
  const [nics, setNics] = useState<NicPower[]>([]);
  const [tcp, setTcp] = useState<TcpLatency[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const busyRef = useRef(new Set<string>());
  const alive = useRef(false);
  const loadPending = useRef(false);

  const load = useCallback(async () => {
    if (loadPending.current) {
      toast("info", "网卡状态正在更新，请稍后再刷新");
      return;
    }
    if (busyRef.current.size > 0) {
      toast("info", "网卡设置正在应用，完成后自动刷新");
      return;
    }
    loadPending.current = true;
    if (alive.current) setLoading(true);
    try {
      const [nResult, tResult] = await Promise.allSettled([api.listNicPower(), api.listTcpLatency()]);
      if (!alive.current) return;
      const failures = [nResult, tResult].filter((r) => r.status === "rejected");
      if (nResult.status === "fulfilled") setNics(nResult.value);
      if (tResult.status === "fulfilled") setTcp(tResult.value);
      setLoadError(failures.length ? "部分网卡设置读取失败；当前列表可能是上次成功数据，切换操作已暂停，请刷新后重试。" : null);
      if (failures.length) toastErr(failures[0].reason);
    } finally {
      loadPending.current = false;
      if (alive.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    alive.current = true;
    void load();
    return () => { alive.current = false; };
  }, [load]);

  const updateNic = async (n: NicPower, enabled: boolean) => {
    if (loadPending.current || loadError || busyRef.current.size > 0) return;
    const key = `nic:${n.guid}`;
    busyRef.current.add(key);
    setBusy(new Set(busyRef.current));
    try {
      await api.setNicPower(n.guid, enabled);
      if (!alive.current) return;
      toastOk(enabled ? "已禁用网卡节能（重插网线/重启网卡后生效）" : "已恢复默认电源管理");
    } catch (e) {
      if (alive.current) toastErr(e);
    } finally {
      busyRef.current.delete(key);
      if (alive.current) {
        setBusy(new Set(busyRef.current));
        await load();
      }
    }
  };

  const updateTcp = async (t: TcpLatency, enabled: boolean) => {
    if (loadPending.current || loadError || busyRef.current.size > 0) return;
    const key = `tcp:${t.guid}`;
    busyRef.current.add(key);
    setBusy(new Set(busyRef.current));
    try {
      await api.setTcpLatency(t.guid, enabled);
      if (!alive.current) return;
      toastOk(enabled ? "TCP 延迟优化已开启" : "TCP 延迟优化已还原");
    } catch (e) {
      if (alive.current) toastErr(e);
    } finally {
      busyRef.current.delete(key);
      if (alive.current) {
        setBusy(new Set(busyRef.current));
        await load();
      }
    }
  };

  return (
    <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
      {busy.size > 0 && (
        <div role="status" aria-live="polite" className="xl:col-span-2 rounded-lg border border-cyan-300/15 bg-cyan-300/5 px-3 py-2 text-[11px] text-cyan-200">
          网卡/TCP 设置正在应用；完成后会自动重新读取状态。
        </div>
      )}
      <Card
        title="网卡节能（防延迟抖动）"
        icon={<Wifi size={15} className="text-emerald-300" />}
        right={
          <Btn variant="ghost" onClick={load} disabled={loading || busy.size > 0}>
            <RefreshCw size={13} className={loading || busy.size > 0 ? "animate-spin" : ""} /> {loading ? "刷新中…" : busy.size > 0 ? "应用中…" : "刷新"}
          </Btn>
        }
      >
        {loadError && (
          <div role="alert" className="mb-2 flex flex-wrap items-start justify-between gap-2 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2">
            <span className="min-w-0 flex-1 break-words text-[11px] text-amber-300">{loadError}</span>
            <Btn variant="ghost" className="shrink-0" disabled={loading} onClick={() => void load()}>
              <RefreshCw size={12} className={loading ? "animate-spin" : ""} /> {loading ? "重试中…" : "重试"}
            </Btn>
          </div>
        )}
        {loading && nics.length === 0 && <div className="text-[12px] text-slate-500">正在读取网卡设置…</div>}
        {nics.map((n) => (
          <div key={n.guid} className="mb-2 flex items-center gap-3 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2.5 text-[13px]">
            <Wifi size={14} className="shrink-0 text-slate-400" />
            <span className="min-w-0 flex-1 truncate text-slate-200">{n.name}</span>
            <Chip tone={n.pm_disabled ? "green" : "amber"}>{n.pm_disabled ? "已禁节能" : "系统默认"}</Chip>
            <Toggle
              label={`${n.name} 网卡节能`}
              on={n.pm_disabled}
              disabled={loading || loadError !== null || busy.size > 0}
              onChange={(v) => { void updateNic(n, v); }}
            />
          </div>
        ))}
        {nics.length === 0 && !loading && !loadError && <div className="text-[12px] text-slate-500">未读取到网卡</div>}
      </Card>

      <Card
        title="TCP 游戏延迟优化"
        icon={<Globe size={15} className="text-rose-300" />}
        right={
          <Btn variant="ghost" onClick={load} disabled={loading || busy.size > 0}>
            <RefreshCw size={13} className={loading || busy.size > 0 ? "animate-spin" : ""} /> {loading ? "刷新中…" : busy.size > 0 ? "应用中…" : "刷新"}
          </Btn>
        }
      >
        {loadError && (
          <div role="alert" className="mb-2 flex flex-wrap items-start justify-between gap-2 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2">
            <span className="min-w-0 flex-1 break-words text-[11px] text-amber-300">{loadError}</span>
            <Btn variant="ghost" className="shrink-0" disabled={loading} onClick={() => void load()}>
              <RefreshCw size={12} className={loading ? "animate-spin" : ""} /> {loading ? "重试中…" : "重试"}
            </Btn>
          </div>
        )}
        {loading && tcp.length === 0 && <div className="mb-2 text-[12px] text-slate-500">正在读取 TCP 参数…</div>}
        <div className="mb-2 text-[11px] leading-4 text-amber-200/80">
          关闭延迟 ACK 与 Nagle 可降低确认延迟，但会轻微降低大文件传输吞吐 — 电竞/竞技场景建议开启
        </div>
        {tcp.map((t) => {
          const on = t.ack_freq === 1 && t.nodelay === 1;
          return (
            <div key={t.guid} className="mb-2 flex items-center gap-3 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2.5 text-[13px]">
              <Globe size={14} className="shrink-0 text-slate-400" />
              <span className="min-w-0 flex-1 truncate text-slate-200">{t.name}</span>
              <Chip tone={on ? "green" : "slate"}>{on ? "已优化" : "默认"}</Chip>
              <Toggle
                label={`${t.name} TCP 延迟优化`}
                on={on}
                disabled={loading || loadError !== null || busy.size > 0}
                onChange={(v) => { void updateTcp(t, v); }}
              />
            </div>
          );
        })}
        {tcp.length === 0 && !loading && !loadError && <div className="text-[12px] text-slate-500">未读取到网卡</div>}
      </Card>
    </div>
  );
}
