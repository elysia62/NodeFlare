import { afterEach, beforeEach, expect, setSystemTime, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { demoServers } from "../demo";
import { useNodeLatency } from "./useNodeLatency";
import type { LatencySample } from "../types";

const now = Date.UTC(2026, 10, 7);
beforeEach(() => setSystemTime(new Date(now)));
afterEach(() => setSystemTime());

function sample(id: string, name: string, latency_ms: number, packet_loss: number): LatencySample {
  return { task_id: id, server_id: "node", name, task_type: "icmp", target: "example.com", port: null,
    timestamp: now / 1000, latency_ms, packet_loss };
}

function summarize(points: LatencySample[]) {
  let result: ReturnType<typeof useNodeLatency> | undefined;
  function Probe() {
    result = useNodeLatency({ ...demoServers[0], latency: points }, false, "zh-CN",
      { telecom: "", unicom: "", mobile: "" });
    return null;
  }
  renderToStaticMarkup(<Probe />);
  return result!;
}

test("complete probe failure displays measured loss and preserves its carrier", () => {
  const result = summarize([sample("ct", "电信", -1, 100)]);
  expect(result.latencyDisplay).toBe("-");
  expect(result.lossDisplay).toBe("100.0%");
  expect(result.lossBars.some(bar => bar.tone === "danger")).toBe(true);
  expect(result.carriers[0].lossDisplay).toBe("100.0%");
  expect(result.carriers[1].lossDisplay).toBe("-");
});

test("failed carriers count toward overall loss without inventing latency", () => {
  const result = summarize([sample("ct", "电信", -1, 100), sample("cu", "联通", 20, 0)]);
  expect(result.latencyDisplay).toBe("20 ms");
  expect(result.lossDisplay).toBe("50.0%");
  expect(result.carriers[0].lossDisplay).toBe("100.0%");
  expect(result.carriers[1].latencyDisplay).toBe("20 ms");
});

test("unobserved probes remain unknown", () => {
  const result = summarize([sample("ct", "电信", -1, -1)]);
  expect(result.latencyDisplay).toBe("-");
  expect(result.lossDisplay).toBe("-");
  expect(result.lossBars.every(bar => bar.tone === "empty")).toBe(true);
});
