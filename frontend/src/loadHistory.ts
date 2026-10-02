import { chartGapLimit, insertTimelineGaps, typicalInterval } from "./chart";
import { number } from "./format";
import { LIVE_CARD_REFRESH_INTERVAL_MS } from "./refresh";
import type { HistoryPoint } from "./types";

const REALTIME_WINDOW_SECONDS = 60 * 60;
// One hour of one-second playback, including both ends of the window.
const MAX_REALTIME_POINTS = REALTIME_WINDOW_SECONDS + 1;

export interface LoadHistoryPoint extends HistoryPoint {
  realtime?: boolean;
}

export interface LoadChartPoint {
  timestamp: number;
  cpu: number | null;
  mem_used: number | null;
  mem_total: number;
  disk_used: number | null;
  disk_total: number;
  net_in: number | null;
  net_out: number | null;
  realtime: boolean;
}

function realtimeWindow(points: readonly LoadHistoryPoint[]): LoadHistoryPoint[] {
  const unique = new Map(points
    .filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0)
    .map((point) => [point.timestamp, point]));
  const sorted = [...unique.values()].sort((left, right) => left.timestamp - right.timestamp);
  const cutoff = (sorted.at(-1)?.timestamp ?? 0) - REALTIME_WINDOW_SECONDS;
  return sorted.filter((point) => point.timestamp >= cutoff).slice(-MAX_REALTIME_POINTS);
}

export function appendRealtimePoint(points: LoadHistoryPoint[], point: HistoryPoint | null): LoadHistoryPoint[] {
  if (!point || !Number.isFinite(point.timestamp) || point.timestamp <= 0) return points;
  return realtimeWindow([...points, { ...point, realtime: true }]);
}

export function mergeLoadHistory(history: HistoryPoint[], current: LoadHistoryPoint[], latest: HistoryPoint | null): LoadHistoryPoint[] {
  // A delayed history response must not erase samples already received live.
  const merged = [
    ...history.map((point) => ({ ...point, realtime: false })),
    ...current.filter((point) => point.realtime),
    ...(latest ? [{ ...latest, realtime: true }] : []),
  ];
  return realtimeWindow(merged);
}

export function loadChartPoints(points: readonly LoadHistoryPoint[], hours: number): LoadChartPoint[] {
  const mapped: LoadChartPoint[] = points
    .filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0)
    .sort((left, right) => left.timestamp - right.timestamp)
    .map((point) => ({
      timestamp: point.timestamp * 1000,
      cpu: Number.isFinite(point.cpu) ? point.cpu : null,
      mem_used: Number.isFinite(point.mem_used) ? point.mem_used : null,
      mem_total: Math.max(0, number(point.mem_total)),
      disk_used: Number.isFinite(point.disk_used) ? point.disk_used : null,
      disk_total: Math.max(0, number(point.disk_total)),
      net_in: Number.isFinite(point.net_in) ? point.net_in : null,
      net_out: Number.isFinite(point.net_out) ? point.net_out : null,
      realtime: point.realtime === true,
    }));
  // History is aggregated (normally one point per minute), while live samples
  // arrive every second. Never infer their gap thresholds from the mixed series.
  const historyInterval = typicalInterval(mapped.filter((point) => !point.realtime)) ?? 60_000;
  return insertTimelineGaps(
    mapped,
    (timestamp) => ({ timestamp, cpu: null, mem_used: null, mem_total: 0, disk_used: null, disk_total: 0, net_in: null, net_out: null, realtime: false }),
    {
      minGap: 10_000,
      maxGap: chartGapLimit(hours),
      intervalForPair: (previous, current) => previous.realtime && current.realtime
        ? LIVE_CARD_REFRESH_INTERVAL_MS : historyInterval,
    },
  );
}
