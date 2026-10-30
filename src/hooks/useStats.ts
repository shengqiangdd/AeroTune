// 订阅实时统计；事件失败时降级为轮询，并对采样中断显示状态。
import { useEffect, useState } from "react";
import { api, Stats } from "../lib/api";
import { validCapacityPair } from "../lib/resourceEvidence";

export interface Hist {
  cpu: number[];
  mem: number[];
  rx: number[];
  tx: number[];
}

export type StatsStatus = "loading" | "live" | "stale" | "error";
const CAP = 120;

function push(arr: number[], v: number): number[] {
  const next = arr.length >= CAP ? arr.slice(arr.length - CAP + 1) : arr.slice();
  next.push(v);
  return next;
}

export function useStats(enabled = true) {
  const [stats, setStats] = useState<Stats | null>(null);
  const [hist, setHist] = useState<Hist>({ cpu: [], mem: [], rx: [], tx: [] });
  const [status, setStatus] = useState<StatsStatus>("loading");
  const [error, setError] = useState("");

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let pending = false;
    let lastSampleId = 0;
    let lastChange = Date.now();
    let fallbackStartedAt = 0;
    let warned = false;
    let pollingFallback = false;

    const report = (e: unknown) => {
      if (disposed) return;
      const message = String(e);
      setError(message);
      if (!warned) {
        console.error("AeroTune stats connection:", e);
        warned = true;
      }
      if (!lastSampleId) setStatus("error");
    };
    const consume = (sample: Stats) => {
      if (disposed) return;
      // sample_id zero denotes the uninitialized backend cache; other metrics remain useful if RAM is unavailable.
      if (!sample.sample_id || !sample.per_core?.length) return;
      const now = Date.now();
      if (sample.sample_id <= lastSampleId) return;
      const wasFallback = pollingFallback;
      lastSampleId = sample.sample_id;
      lastChange = now;
      pollingFallback = false;
      fallbackStartedAt = 0;
      if (wasFallback) console.info("AeroTune stats recovered from event silence");
      setStats(sample);
      setStatus("live");
      setError("");
      warned = false;
      setHist((h) => ({
        cpu: push(h.cpu, sample.cpu),
        mem: validCapacityPair(sample.mem_used_gb, sample.mem_total_gb)
          ? push(h.mem, (sample.mem_used_gb / sample.mem_total_gb) * 100)
          : h.mem,
        rx: push(h.rx, sample.net_rx_kbps),
        tx: push(h.tx, sample.net_tx_kbps),
      }));
    };
    const poll = async () => {
      if (pending || disposed) return;
      pending = true;
      try {
        consume(await api.getStats());
      } catch (e) {
        report(e);
      } finally {
        pending = false;
      }
    };
    poll();
    api.onStats(consume).then((stop) => {
      if (disposed) stop();
      else {
        unlisten = stop;
        // Keep a lightweight health check: issue a command poll only after event silence.
        pollingFallback = false;
      }
    }).catch((e) => {
      pollingFallback = true;
      report(e);
      poll();
    });
    const timer = setInterval(() => {
      if (document.hidden) return;
      const silentFor = Date.now() - lastChange;
      if (silentFor > 10000) {
        pollingFallback = true;
        setStatus("stale");
      }
      if (pollingFallback && silentFor > 3000) poll();
    }, 3000);
    return () => { disposed = true; clearInterval(timer); unlisten?.(); };
  }, [enabled]);

  return { stats, hist, status, error };
}
