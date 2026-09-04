import { clockLabel } from "./chart";
import { ui, type UiLocale } from "./locale";
import type { LatencySample } from "./types";

type LatencyTone = "good" | "fair" | "warning" | "poor" | "danger" | "empty";

export interface LatencyBar {
  key: string;
  tone: LatencyTone;
  tooltip: string;
}

export interface CarrierSlotKey {
  telecom: string;
  mobile: string;
  unicom: string;
}

export const BAR_COUNT = 20;

/**
 * Carrier lines, in display order. Aliases cover the Chinese names plus the
 * common latin spellings and operator codes, so an admin who names a task
 * "CT-Guangzhou" or "电信广州" lands in the same slot without extra config.
 */
export const CARRIER_SLOTS = [
  { key: "telecom" as const, label: "电信", labelEn: "Telecom", color: "#fb7185", aliases: ["电信", "chinatelecom", "telecom", "ctcc", "ct"] },
  { key: "mobile" as const, label: "移动", labelEn: "Mobile", color: "#34d399", aliases: ["移动", "chinamobile", "mobile", "cmcc", "cm"] },
  { key: "unicom" as const, label: "联通", labelEn: "Unicom", color: "#60a5fa", aliases: ["联通", "chinaunicom", "unicom", "cucc", "cu"] },
] as const;

export type CarrierSlot = typeof CARRIER_SLOTS[number];

function latencyTone(value: number): LatencyTone {
  if (value <= 60) return "good";
  if (value <= 100) return "fair";
  if (value <= 160) return "warning";
  if (value <= 200) return "poor";
  return "danger";
}

function lossTone(value: number): LatencyTone {
  if (value <= 1) return "good";
  if (value <= 3) return "fair";
  if (value <= 6) return "warning";
  if (value <= 9) return "poor";
  return "danger";
}

export function emptyBars(label: string): LatencyBar[] {
  return Array.from({ length: BAR_COUNT }, (_, index) => ({ key: `empty-${index}`, tone: "empty" as const, tooltip: label }));
}

function normalizeTaskName(name: string): string {
  return name.normalize("NFKC").toLowerCase().replace(/[\s_·、,，/|-]+/g, "");
}

export interface LatencyTaskRef {
  id: string;
  name: string;
}

/**
 * Resolve the three carrier slots. Explicit names win; when every name is
 * blank we match on the task name, then fill any slot still empty using the
 * backend order so a two-task setup still renders two lines.
 */
export function selectCarrierTasks(
  tasks: readonly LatencyTaskRef[],
  configured: CarrierSlotKey,
): Map<CarrierSlot["key"], LatencyTaskRef> {
  const selected = new Map<CarrierSlot["key"], LatencyTaskRef>();
  const used = new Set<string>();
  const names = CARRIER_SLOTS.map((slot) => configured[slot.key].trim());
  const explicit = names.some((name) => name !== "");

  if (explicit) {
    const byName = new Map<string, LatencyTaskRef>();
    for (const task of tasks) {
      const name = task.name.trim();
      if (name && !byName.has(name)) byName.set(name, task);
    }
    CARRIER_SLOTS.forEach((slot, index) => {
      const task = names[index] ? byName.get(names[index]) : undefined;
      if (!task || used.has(task.id)) return;
      selected.set(slot.key, task);
      used.add(task.id);
    });
    return selected;
  }

  const available = tasks.map((task) => ({ task, normalized: normalizeTaskName(task.name) }));
  for (const slot of CARRIER_SLOTS) {
    const match = available.find((candidate) => !used.has(candidate.task.id)
      && slot.aliases.some((alias) => candidate.normalized.includes(alias)));
    if (!match) continue;
    selected.set(slot.key, match.task);
    used.add(match.task.id);
  }
  for (const slot of CARRIER_SLOTS) {
    if (selected.has(slot.key)) continue;
    const match = available.find((candidate) => !used.has(candidate.task.id));
    if (!match) break;
    selected.set(slot.key, match.task);
    used.add(match.task.id);
  }
  return selected;
}

export interface LatencyBucket {
  timestamp: number;
  latency: number | null;
  loss: number | null;
}

function validValue(value: number): number | null {
  return Number.isFinite(value) && value >= 0 ? value : null;
}

/**
 * Bucket samples across the requested window rather than across the extent of
 * whatever data came back. A node that only reported for the last five minutes
 * of an hour shows fifteen empty buckets instead of twenty healthy ones.
 *
 * The grid always spans exactly `windowSeconds`, so a bar's position means the
 * same thing on every card. When a task samples more slowly than one bucket,
 * its last reading carries forward for one cadence — otherwise a 10-minute
 * task would alternate between filled and empty bars and read as flapping.
 */
export function bucketSamples(points: readonly LatencySample[], windowSeconds: number, now = Date.now() / 1000): LatencyBucket[] {
  const samples = points
    .filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0)
    .map((point) => ({ timestamp: point.timestamp, latency: validValue(point.latency_ms), loss: validValue(point.packet_loss) }))
    .filter((point) => point.latency !== null || point.loss !== null)
    .sort((left, right) => left.timestamp - right.timestamp);
  if (!samples.length) return [];

  const deltas = samples
    .slice(1)
    .map((sample, index) => sample.timestamp - samples[index].timestamp)
    .filter((delta) => delta > 0 && Number.isFinite(delta))
    .sort((left, right) => left - right);
  const cadence = deltas.length ? deltas[Math.floor((deltas.length - 1) / 4)] : 0;
  const span = Math.max(1, windowSeconds);
  const bucketSize = span / BAR_COUNT;
  const end = Math.max(now, samples[samples.length - 1].timestamp);
  const start = end - span;
  // Only bridge buckets when the task genuinely samples slower than the grid.
  const carryFor = cadence > bucketSize ? cadence * 1.5 : 0;

  const buckets: LatencyBucket[] = [];
  let cursor = 0;
  let carried: LatencyBucket | null = null;
  let carriedAt = -Infinity;
  while (cursor < samples.length && samples[cursor].timestamp < start) {
    carried = { timestamp: 0, latency: samples[cursor].latency, loss: samples[cursor].loss };
    carriedAt = samples[cursor].timestamp;
    cursor += 1;
  }

  for (let index = 0; index < BAR_COUNT; index += 1) {
    const bucketStart = start + bucketSize * index;
    // The final bucket includes its upper bound so a sample landing exactly on
    // `end` is not dropped.
    const bucketEnd = bucketStart + bucketSize;
    const inclusive = index === BAR_COUNT - 1;
    let latencySum = 0;
    let latencyCount = 0;
    let lossSum = 0;
    let lossCount = 0;
    while (cursor < samples.length
      && (inclusive ? samples[cursor].timestamp <= bucketEnd : samples[cursor].timestamp < bucketEnd)) {
      const sample = samples[cursor];
      if (sample.latency !== null) { latencySum += sample.latency; latencyCount += 1; }
      if (sample.loss !== null) { lossSum += sample.loss; lossCount += 1; }
      carriedAt = sample.timestamp;
      cursor += 1;
    }
    if (latencyCount || lossCount) {
      const bucket = {
        timestamp: Math.round(bucketStart),
        latency: latencyCount ? latencySum / latencyCount : null,
        loss: lossCount ? lossSum / lossCount : null,
      };
      carried = bucket;
      buckets.push(bucket);
      continue;
    }
    const bridged = carryFor > 0 && carried !== null && bucketStart - carriedAt <= carryFor;
    buckets.push({
      timestamp: Math.round(bucketStart),
      latency: bridged ? carried!.latency : null,
      loss: bridged ? carried!.loss : null,
    });
  }
  return buckets;
}

export function latencyBars(buckets: readonly LatencyBucket[], prefix: string, locale: UiLocale): LatencyBar[] {
  const missing = ui(locale, "无采样数据", "No samples");
  return buckets.map((bucket, index) => ({
    key: `${prefix}-latency-${bucket.timestamp}-${index}`,
    tone: bucket.latency === null ? "empty" as const : latencyTone(bucket.latency),
    tooltip: `${clockLabel(bucket.timestamp * 1000, locale)}\n${bucket.latency === null ? missing : `${Math.round(bucket.latency)} ms`}`,
  }));
}

export function lossBars(buckets: readonly LatencyBucket[], prefix: string, locale: UiLocale): LatencyBar[] {
  const missing = ui(locale, "无采样数据", "No samples");
  return buckets.map((bucket, index) => ({
    key: `${prefix}-loss-${bucket.timestamp}-${index}`,
    tone: bucket.loss === null ? "empty" as const : lossTone(bucket.loss),
    tooltip: `${clockLabel(bucket.timestamp * 1000, locale)}\n${bucket.loss === null ? missing : `${bucket.loss.toFixed(1)}%`}`,
  }));
}

export function averageOf(values: readonly number[]): number | null {
  const valid = values.filter((value) => Number.isFinite(value) && value >= 0);
  return valid.length ? valid.reduce((sum, value) => sum + value, 0) / valid.length : null;
}
