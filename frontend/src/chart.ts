import type { UiLocale } from "./locale";

interface TimedPoint {
  timestamp: number;
}

/**
 * Lower quartile of the observed sampling deltas. A quartile keeps repeated
 * outages from inflating what we treat as the normal cadence, which is what
 * decides where a timeline break belongs.
 */
export function typicalInterval(points: readonly TimedPoint[]): number | null {
  const intervals = points
    .slice(1)
    .map((point, index) => point.timestamp - points[index].timestamp)
    .filter((interval) => interval > 0 && Number.isFinite(interval))
    .sort((left, right) => left - right);
  return intervals.length ? intervals[Math.floor((intervals.length - 1) / 4)] : null;
}

/**
 * Insert a break point whenever the timeline skips more than the expected
 * cadence, so a chart never draws a straight line across an outage.
 * `minGap`/`maxGap` use the same unit as `timestamp`.
 */
export function insertTimelineGaps<T extends TimedPoint>(
  points: readonly T[],
  createGap: (timestamp: number) => T,
  { minGap, maxGap }: { minGap: number; maxGap: number },
): T[] {
  if (points.length < 2) return [...points];
  const interval = typicalInterval(points);
  if (interval === null) return [...points];
  const threshold = Math.min(Math.max(minGap, interval * 1.5), maxGap);
  const result: T[] = [points[0]];
  for (let index = 1; index < points.length; index += 1) {
    const previous = points[index - 1];
    const current = points[index];
    if (current.timestamp - previous.timestamp > threshold) {
      result.push(createGap(previous.timestamp + interval));
    }
    result.push(current);
  }
  return result;
}

/** Widest break a chart of this span should draw, in milliseconds. */
export function chartGapLimit(hours: number): number {
  if (hours <= 0) return 30_000;
  return Math.max(5 * 60_000, (hours * 60 * 60_000) / 36);
}

/** Fixed axis window so a partially populated range keeps its true scale. */
export function chartTimeRange(hours: number, now = Date.now()) {
  const start = now - hours * 60 * 60_000;
  return {
    domain: [start, now] as [number, number],
    ticks: Array.from({ length: 5 }, (_, index) => start + ((now - start) * index) / 4),
  };
}

export function chartTimeLabel(timestamp: number, hours: number, locale: UiLocale): string {
  const date = new Date(timestamp);
  if (hours <= 1) return date.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
  if (hours <= 4) return date.toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false });
  if (hours <= 24) return date.toLocaleString(locale, { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });
  return date.toLocaleDateString(locale, { month: "2-digit", day: "2-digit" });
}

export function clockLabel(timestamp: number, locale: UiLocale): string {
  return new Date(timestamp).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit", hour12: false });
}
