import { describe, expect, test } from "bun:test";
import { demoHistory } from "./demo";
import { appendRealtimePoint, loadChartPoints, mergeLoadHistory, type LoadHistoryPoint } from "./loadHistory";
import type { HistoryPoint } from "./types";

const start = 1_788_886_800;
const metrics = demoHistory("load-regression", 1)[0];
function point(second: number, cpu = 30): HistoryPoint {
  return { ...metrics, timestamp: start + second, cpu };
}

describe("load chart history and live playback", () => {
  test("minute history stays connected after 13 seconds and sustained one-second updates", () => {
    const history = Array.from({ length: 40 }, (_, index) => point(index * 60));
    let points: LoadHistoryPoint[] = history;
    for (let second = 1; second <= 90; second++) {
      points = appendRealtimePoint(points, point(39 * 60 + second));
      const rendered = loadChartPoints(points, 1);
      expect(rendered).toHaveLength(40 + second);
      expect(rendered.filter((item) => item.cpu === null)).toHaveLength(0);
      expect(rendered.slice(0, 40).map((item) => item.timestamp)).toEqual(history.map((item) => item.timestamp * 1000));
    }
  });

  test("one to three historical points do not borrow the shorter live cadence", () => {
    for (const count of [1, 2, 3]) {
      let points: LoadHistoryPoint[] = Array.from({ length: count }, (_, index) => point(index * 60));
      for (let second = 1; second <= 30; second++) {
        points = appendRealtimePoint(points, point((count - 1) * 60 + second));
      }
      expect(loadChartPoints(points, 1)).toHaveLength(count + 30);
    }
  });

  test("actual historical and live outages remain separate gaps", () => {
    let points: LoadHistoryPoint[] = [0, 60, 120, 720, 780].map((second) => point(second));
    for (const second of [781, 782, 783, 784, 785, 825, 826]) points = appendRealtimePoint(points, point(second));
    const rendered = loadChartPoints(points, 1);
    const gaps = rendered.filter((item) => item.cpu === null);
    expect(gaps).toHaveLength(2);
    expect(gaps[0].timestamp).toBeGreaterThan((start + 120) * 1000);
    expect(gaps[0].timestamp).toBeLessThan((start + 720) * 1000);
    expect(gaps[1].timestamp).toBeGreaterThan((start + 785) * 1000);
    expect(gaps[1].timestamp).toBeLessThan((start + 825) * 1000);
    expect(gaps.every((item) => item.mem_used === null && item.disk_used === null && item.net_in === null && item.net_out === null)).toBe(true);
  });

  test("a long gap from the last history point to the first live sample stays visible", () => {
    let points: LoadHistoryPoint[] = [0, 60, 120].map((second) => point(second));
    for (let second = 1; second <= 30; second++) points = appendRealtimePoint(points, point(720 + second));
    expect(loadChartPoints(points, 1).filter((item) => item.cpu === null)).toHaveLength(1);
  });

  test("ordinary historical ranges retain their own sampling interval", () => {
    const points = [0, 600, 1200, 1800, 7200, 7800].map((second) => point(second));
    const rendered = loadChartPoints(points, 24);
    expect(rendered).toHaveLength(points.length + 1);
    expect(rendered.filter((item) => item.cpu === null)).toHaveLength(1);
  });

  test("the old 720-point limit cannot evict history after twelve minutes", () => {
    let points: LoadHistoryPoint[] = Array.from({ length: 40 }, (_, index) => point(index * 60));
    for (let second = 1; second <= 900; second++) points = appendRealtimePoint(points, point(39 * 60 + second));
    expect(points).toHaveLength(940);
    expect(points[0].timestamp).toBe(start);
    expect(points.filter((item) => !item.realtime)).toHaveLength(40);
    expect(loadChartPoints(points, 1).filter((item) => item.cpu === null)).toHaveLength(0);
  });

  test("retention is bounded by one full hour at one-second cadence", () => {
    const fullHour = Array.from({ length: 3601 }, (_, second) => ({ ...point(second), realtime: true }));
    const points = appendRealtimePoint(fullHour, point(3601));
    expect(points).toHaveLength(3601);
    expect(points[0].timestamp).toBe(start + 1);
    expect(points.at(-1)?.timestamp).toBe(start + 3601);
    const stale = appendRealtimePoint(points, point(0));
    expect(stale).toEqual(points);
    expect(fullHour[0].timestamp).toBe(start);
  });

  test("a delayed history response merges live samples without overwriting them", () => {
    const history = [0, 60, 120].map((second) => point(second));
    let current: LoadHistoryPoint[] = [point(30, 99)];
    for (let second = 121; second <= 140; second++) current = appendRealtimePoint(current, point(second, 40));
    const merged = mergeLoadHistory(history, current, point(140, 50));
    expect(merged).toHaveLength(23);
    expect(merged.some((item) => item.timestamp === start + 30)).toBe(false);
    expect(merged.filter((item) => item.realtime)).toHaveLength(20);
    expect(merged.at(-1)?.cpu).toBe(50);
    expect(loadChartPoints(merged, 1)).toHaveLength(23);
  });

  test("duplicates prefer live values and invalid timestamps do not disturb the timeline", () => {
    const original = Object.freeze(point(60, 10));
    const points = appendRealtimePoint([original], point(60, 50));
    expect(points).toHaveLength(1);
    expect(points[0]).toMatchObject({ cpu: 50, realtime: true });
    expect(original.cpu).toBe(10);
    expect(appendRealtimePoint(points, null)).toBe(points);
    expect(appendRealtimePoint(points, { ...point(0), timestamp: NaN })).toBe(points);
    expect(appendRealtimePoint(points, { ...point(0), timestamp: 0 })).toBe(points);
    expect(loadChartPoints([], 1)).toEqual([]);
    expect(loadChartPoints([original], 1)).toHaveLength(1);
  });
});
