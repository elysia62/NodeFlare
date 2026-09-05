import type { LatencySample, LiveLatencyResult, Server } from "./types";

interface LiveSample {
  ts: number;
  data: Partial<Server> & { latency_results?: LiveLatencyResult[] };
}

export interface LiveMetrics {
  timestamp: number;
  metrics: Partial<Server>;
  latencyResults?: LiveLatencyResult[];
}

export type LiveMetricsMap = Record<string, LiveMetrics>;

export interface BatchUpdate {
  serverId?: string;
  samples?: LiveSample[];
}

const MAX_LIVE_LATENCY_RESULTS = 4096;

export function mergeLiveResults(
  previous: LiveLatencyResult[] | undefined,
  incoming: readonly LiveLatencyResult[],
): LiveLatencyResult[] {
  const results = new Map<string, LiveLatencyResult>();
  for (const result of [...(previous ?? []), ...incoming]) {
    if (!result?.task_id || !Number.isFinite(result.timestamp) || result.timestamp <= 0
      || !Number.isFinite(result.latency_ms) || !Number.isFinite(result.packet_loss)) continue;
    results.set(`${result.task_id}:${result.timestamp}`, result);
  }
  const merged = Array.from(results.values())
    .sort((left, right) => left.timestamp - right.timestamp)
    .slice(-MAX_LIVE_LATENCY_RESULTS);
  return previous && merged.length === previous.length && merged.every((result, index) => {
    const before = previous[index];
    return result.task_id === before.task_id && result.timestamp === before.timestamp
      && result.latency_ms === before.latency_ms && result.packet_loss === before.packet_loss;
  }) ? previous : merged;
}

export function liveLatencySamples(
  definitions: readonly LatencySample[],
  results: readonly LiveLatencyResult[] = [],
): LatencySample[] {
  const tasks = new Map(definitions.map((definition) => [definition.task_id, definition]));
  return results.flatMap((result) => {
    const definition = tasks.get(result.task_id);
    return definition ? [{ ...definition, ...result }] : [];
  });
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

function sampleMetrics(sample: LiveSample) {
  const metrics = { ...sample.data };
  delete metrics.latency_results;
  return metrics;
}

export function applyBatch(
  current: LiveMetricsMap,
  updates: readonly BatchUpdate[],
  servers: readonly Server[],
): LiveMetricsMap {
  let next = current;
  const knownServers = new Map(servers.map((server) => [server.id, server]));
  for (const update of updates) {
    if (!update?.serverId || !Array.isArray(update.samples) || !update.samples.length) continue;
    const server = knownServers.get(update.serverId);
    if (!server) continue;
    const samples = update.samples
      .filter((sample) => sample && Number.isFinite(sample.ts) && sample.ts > 0
        && sample.data && typeof sample.data === "object" && !Array.isArray(sample.data))
      .sort((left, right) => left.ts - right.ts);
    if (!samples.length) continue;
    const previous = next[update.serverId];
    const latest = samples[samples.length - 1];
    const newer = latest.ts > Math.max(previous?.timestamp ?? 0, server.timestamp ?? 0);
    // Metrics use the newest snapshot; latency history retains results from the whole batch.
    const incomingLatency = samples.flatMap((sample) => Array.isArray(sample.data.latency_results)
      ? sample.data.latency_results : []);
    const latencyResults = incomingLatency.length
      ? mergeLiveResults(previous?.latencyResults, incomingLatency)
      : previous?.latencyResults;
    if (!newer && latencyResults === previous?.latencyResults) continue;
    if (next === current) next = { ...current };
    next[update.serverId] = {
      timestamp: newer ? latest.ts : previous?.timestamp ?? 0,
      metrics: newer ? sampleMetrics(latest) : previous?.metrics ?? {},
      latencyResults,
    };
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

export function pruneLiveMetrics(current: LiveMetricsMap, servers: readonly Server[]): LiveMetricsMap {
  const ids = new Set(servers.map((server) => server.id));
  const entries = Object.entries(current).filter(([id]) => ids.has(id));
  return entries.length === Object.keys(current).length ? current : Object.fromEntries(entries);
}
