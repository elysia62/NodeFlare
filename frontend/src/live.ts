import type { LatencySample, LiveLatencyResult, Server } from "./types";

interface LiveSample {
  ts: number;
  data: Partial<Server> & { latency_results?: LiveLatencyResult[] };
}

export interface LiveMetrics {
  timestamp: number;
  displayTimestamp: number;
  metrics: Partial<Server>;
  latencyResults?: LiveLatencyResult[];
}

export type LiveMetricsMap = Record<string, LiveMetrics>;

export type PlaybackBuffer = Map<string, LiveSample[]>;

export interface BatchUpdate {
  serverId?: string;
  samples?: LiveSample[];
  reportAgeMs?: number;
}

const MAX_PLAYBACK_SAMPLES_PER_SERVER = 600;
const MAX_LIVE_LATENCY_RESULTS = 4096;

export function mergeLiveResults(
  previous: LiveLatencyResult[] | undefined,
  incoming: LiveLatencyResult[],
): LiveLatencyResult[] {
  const results = new Map<string, LiveLatencyResult>();
  for (const result of [...(previous ?? []), ...incoming]) {
    if (!result.task_id || !Number.isFinite(result.timestamp) || result.timestamp <= 0) continue;
    results.set(`${result.task_id}:${result.timestamp}`, result);
  }
  return Array.from(results.values())
    .sort((left, right) => left.timestamp - right.timestamp)
    .slice(-MAX_LIVE_LATENCY_RESULTS);
}

export function mergeLiveLatency(server: Server, results: LiveLatencyResult[]): LatencySample[] {
  const latest = new Map<string, LiveLatencyResult>();
  for (const result of results) {
    const current = latest.get(result.task_id);
    if (!current || result.timestamp >= current.timestamp) latest.set(result.task_id, result);
  }
  let changed = false;
  const merged = server.latency.map((definition) => {
    const result = latest.get(definition.task_id);
    if (!result || result.timestamp <= definition.timestamp) return definition;
    changed = true;
    return {
      ...definition,
      server_id: server.id,
      timestamp: result.timestamp,
      latency_ms: result.latency_ms,
      packet_loss: result.packet_loss,
    };
  });
  return changed ? merged : server.latency;
}

function splitSample(sample: LiveSample) {
  const metrics = { ...sample.data };
  const latencyResults = Array.isArray(metrics.latency_results) ? metrics.latency_results : [];
  delete metrics.latency_results;
  return { metrics, latencyResults };
}

export function applyBatch(
  current: LiveMetricsMap,
  updates: readonly BatchUpdate[],
  context: { cached: boolean; playback: PlaybackBuffer; servers: readonly Server[] },
): LiveMetricsMap {
  const next = { ...current };
  for (const update of updates) {
    if (!update.serverId || !Array.isArray(update.samples) || !update.samples.length) continue;
    const samples = update.samples
      .filter((sample) => Number.isFinite(sample.ts) && sample.data && typeof sample.data === "object")
      .sort((left, right) => left.ts - right.ts);
    const previous = next[update.serverId];
    const pending = context.playback.get(update.serverId) ?? [];
    const persistedTimestamp = context.servers.find((server) => server.id === update.serverId)?.timestamp ?? 0;
    const appliedTimestamp = Math.max(previous?.timestamp ?? 0, persistedTimestamp);
    const seen = new Set<number>(pending.map((sample) => sample.ts));
    const incoming = samples.filter((sample) => sample.ts > appliedTimestamp && !seen.has(sample.ts));
    if (!incoming.length) continue;

    const reportAgeSeconds = context.cached && Number.isFinite(update.reportAgeMs)
      ? Math.max(0, update.reportAgeMs! / 1000)
      : 0;
    const cursor = context.cached
      ? Math.max(previous?.displayTimestamp ?? 0, incoming[incoming.length - 1].ts + reportAgeSeconds)
      : previous?.displayTimestamp ?? incoming[0].ts;
    const all = [...pending, ...incoming]
      .sort((left, right) => left.ts - right.ts)
      .slice(-MAX_PLAYBACK_SAMPLES_PER_SERVER);
    let selected: LiveSample | undefined;
    while (all.length && all[0].ts <= cursor) selected = all.shift();
    if (selected) {
      const { metrics, latencyResults } = splitSample(selected);
      next[update.serverId] = {
        timestamp: selected.ts,
        displayTimestamp: cursor,
        metrics,
        latencyResults: latencyResults.length
          ? mergeLiveResults(previous?.latencyResults, latencyResults)
          : previous?.latencyResults,
      };
    } else if (!previous) {
      continue;
    }
    if (all.length) context.playback.set(update.serverId, all);
    else context.playback.delete(update.serverId);
  }
  return next;
}

export function advancePlayback(
  current: LiveMetricsMap,
  playback: PlaybackBuffer,
  elapsedSeconds: number,
): LiveMetricsMap {
  if (!playback.size) return current;
  const next = { ...current };
  for (const [serverId, samples] of playback) {
    const state = next[serverId];
    if (!state || !samples.length) continue;
    const displayTimestamp = state.displayTimestamp + elapsedSeconds;
    let selected: LiveSample | undefined;
    while (samples.length && samples[0].ts <= displayTimestamp) selected = samples.shift();
    if (selected) {
      const { metrics, latencyResults } = splitSample(selected);
      next[serverId] = {
        timestamp: selected.ts,
        displayTimestamp,
        metrics,
        latencyResults: latencyResults.length
          ? mergeLiveResults(state.latencyResults, latencyResults)
          : state.latencyResults,
      };
    } else {
      next[serverId] = { ...state, displayTimestamp };
    }
    if (!samples.length) playback.delete(serverId);
  }
  return next;
}

export function mergeServerLive(
  server: Server,
  live: LiveMetrics | undefined,
  clockNow: number,
  offlineThresholdSeconds: number,
): Server {
  const merged = !live ? server : (() => {
    const latency = live.latencyResults?.length ? mergeLiveLatency(server, live.latencyResults) : server.latency;
    if (live.timestamp <= (server.timestamp ?? 0)) {
      return latency === server.latency ? server : { ...server, latency };
    }
    return { ...server, ...live.metrics, latency, timestamp: live.timestamp };
  })();
  if (!merged.timestamp || !merged.uptime || !Number.isFinite(merged.uptime)) return merged;
  const elapsed = Math.max(0, Math.floor(clockNow / 1000 - merged.timestamp));
  return elapsed > 0 && elapsed <= offlineThresholdSeconds
    ? { ...merged, uptime: merged.uptime + elapsed }
    : merged;
}

export function pruneStalePlayback(playback: PlaybackBuffer, servers: readonly Server[]) {
  for (const server of servers) {
    const samples = playback.get(server.id);
    if (!samples || !server.timestamp) continue;
    const fresh = samples.filter((sample) => sample.ts > server.timestamp!);
    if (fresh.length) playback.set(server.id, fresh);
    else playback.delete(server.id);
  }
}
