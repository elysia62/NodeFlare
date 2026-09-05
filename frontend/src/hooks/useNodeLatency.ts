import { useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { liveLatencySamples } from "../live";
import {
  averageOf,
  bucketSamples,
  CARRIER_SLOTS,
  emptyBars,
  latencyBars,
  lossBars,
  selectCarrierTasks,
  type CarrierSlotKey,
  type LatencyBar,
  type LatencyTaskRef,
} from "../latency";
import { ui, type UiLocale } from "../locale";
import type { LatencySample, LiveLatencyResult, Server } from "../types";

export type { LatencyBar } from "../latency";

export interface CarrierLatencyRow {
  id: string;
  label: string;
  name: string;
  color: string;
  latencyDisplay: string;
  lossDisplay: string;
  latencyBars: LatencyBar[];
  lossBars: LatencyBar[];
}

export interface NodeLatencyStats {
  latencyDisplay: string;
  lossDisplay: string;
  latencyBars: LatencyBar[];
  lossBars: LatencyBar[];
  carriers: CarrierLatencyRow[];
  loading: boolean;
}

const cache = new Map<string, { at: number; signature: string; points: LatencySample[] }>();
const CACHE_TTL = 120_000;
const REFRESH_INTERVAL = 120_000;
const WINDOW_HOURS = 1;

function mergePoints(...sources: LatencySample[][]): LatencySample[] {
  const points = new Map<string, LatencySample>();
  for (const source of sources) {
    for (const point of source) {
      if (!Number.isFinite(point.timestamp) || point.timestamp <= 0) continue;
      points.set(`${point.task_id}:${point.timestamp}`, point);
    }
  }
  return Array.from(points.values()).sort((left, right) => left.timestamp - right.timestamp);
}

function taskRefs(points: readonly LatencySample[]): LatencyTaskRef[] {
  const tasks = new Map<string, LatencyTaskRef>();
  for (const point of points) {
    if (!point.task_id || tasks.has(point.task_id)) continue;
    tasks.set(point.task_id, { id: point.task_id, name: point.name ?? "" });
  }
  return Array.from(tasks.values());
}

export function useNodeLatency(
  server: Server,
  enabled: boolean,
  locale: UiLocale,
  carrierSelection: CarrierSlotKey | null = null,
  liveResults?: LiveLatencyResult[],
): NodeLatencyStats {
  const [fetched, setFetched] = useState<LatencySample[]>(server.latency);
  const [loading, setLoading] = useState(enabled);

  const taskSignature = [...new Set(server.latency.map((point) => point.task_id))].sort().join(",");

  useEffect(() => {
    setFetched(server.latency);
  }, [server.id, taskSignature]);

  useEffect(() => {
    if (!enabled) { setLoading(false); return; }
    let stopped = false;
    let running = false;
    let timer: number | undefined;

    const canRefresh = () => !document.hidden && navigator.onLine !== false;
    const clearTimer = () => {
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
    };
    const schedule = () => {
      clearTimer();
      if (!stopped && canRefresh()) {
        timer = window.setTimeout(() => void load(true), REFRESH_INTERVAL);
      }
    };
    const load = async (force: boolean) => {
      clearTimer();
      if (stopped || running || !canRefresh()) return;
      running = true;
      try {
        const hit = cache.get(server.id);
        if (!force && hit?.signature === taskSignature && Date.now() - hit.at < CACHE_TTL) {
          setFetched(hit.points);
          setLoading(false);
          return;
        }
        if (!hit) setLoading(true);
        const result = await api.latencyHistory(server.id, WINDOW_HOURS);
        if (!stopped) {
          cache.set(server.id, { at: Date.now(), signature: taskSignature, points: result.points });
          setFetched(result.points);
        }
      } catch {} finally {
        running = false;
        if (!stopped) setLoading(false);
        schedule();
      }
    };
    const resume = () => {
      clearTimer();
      if (!stopped && canRefresh() && !running) void load(false);
    };
    const pause = () => clearTimer();
    const handleVisibility = () => {
      if (document.hidden) pause();
      else resume();
    };

    document.addEventListener("visibilitychange", handleVisibility);
    window.addEventListener("online", resume);
    window.addEventListener("offline", pause);
    if (canRefresh()) void load(false);
    return () => {
      stopped = true;
      clearTimer();
      document.removeEventListener("visibilitychange", handleVisibility);
      window.removeEventListener("online", resume);
      window.removeEventListener("offline", pause);
    };
  }, [enabled, server.id, taskSignature]);

  const points = useMemo(() => {
    const cutoff = Date.now() / 1000 - WINDOW_HOURS * 3600;
    return mergePoints(fetched, server.latency, liveLatencySamples(server.latency, liveResults))
      .filter((point) => point.timestamp >= cutoff);
  }, [server.latency, fetched, liveResults]);
  const carrierKey = carrierSelection
    ? `${carrierSelection.telecom}\0${carrierSelection.mobile}\0${carrierSelection.unicom}`
    : "";

  return useMemo<NodeLatencyStats>(() => {
    const windowSeconds = WINDOW_HOURS * 3600;
    const placeholder = loading ? ui(locale, "加载中", "Loading") : ui(locale, "无采样数据", "No samples");

    const byTask = new Map<string, LatencySample[]>();
    for (const point of points) {
      const samples = byTask.get(point.task_id) ?? [];
      samples.push(point);
      byTask.set(point.task_id, samples);
    }
    const usable = new Set([...byTask.entries()]
      .filter(([, samples]) => samples.some((sample) => Number.isFinite(sample.latency_ms) && sample.latency_ms >= 0))
      .map(([taskId]) => taskId));
    const included = points.filter((point) => usable.has(point.task_id));
    const buckets = bucketSamples(included, windowSeconds);
    const averageLatency = averageOf(included.map((point) => point.latency_ms));
    const averageLoss = averageOf(included.map((point) => point.packet_loss));

    const carriers: CarrierLatencyRow[] = [];
    if (carrierSelection) {
      const available = taskRefs(points).filter((task) => usable.has(task.id));
      const selected = selectCarrierTasks(available, carrierSelection);
      for (const slot of CARRIER_SLOTS) {
        const task = selected.get(slot.key);
        if (!task) continue;
        const samples = byTask.get(task.id) ?? [];
        const taskBuckets = bucketSamples(samples, windowSeconds);
        const latency = averageOf(samples.map((sample) => sample.latency_ms));
        const loss = averageOf(samples.map((sample) => sample.packet_loss));
        carriers.push({
          id: task.id,
          label: ui(locale, slot.label, slot.labelEn),
          name: task.name || ui(locale, slot.label, slot.labelEn),
          color: slot.color,
          latencyDisplay: latency === null ? "-" : `${Math.round(latency)} ms`,
          lossDisplay: loss === null ? "-" : `${loss.toFixed(1)}%`,
          latencyBars: taskBuckets.length ? latencyBars(taskBuckets, task.id, locale) : emptyBars(placeholder),
          lossBars: taskBuckets.length ? lossBars(taskBuckets, task.id, locale) : emptyBars(placeholder),
        });
      }
    }

    return {
      latencyDisplay: averageLatency === null ? (loading ? placeholder : "-") : `${Math.round(averageLatency)} ms`,
      lossDisplay: averageLoss === null ? (loading ? placeholder : "-") : `${averageLoss.toFixed(1)}%`,
      latencyBars: buckets.length ? latencyBars(buckets, "all", locale) : emptyBars(placeholder),
      lossBars: buckets.length ? lossBars(buckets, "all", locale) : emptyBars(placeholder),
      carriers,
      loading,
    };
  }, [carrierKey, carrierSelection !== null, loading, locale, points]);
}
