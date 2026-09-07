import { describe, expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const script = readFileSync(new URL("../../scripts/smoke-test.sh", import.meta.url), "utf8");
const assertion = script.match(/^assert_latency_history\(\) \{[\s\S]*?^\}/m)?.[0];
if (!assertion) throw new Error("Missing latency history smoke assertion");

const taskId = "smoke-latency";
const point = (timestamp: number, latency_ms: number, packet_loss: number, task_id = taskId) => ({
  task_id, timestamp, latency_ms, packet_loss,
});

function check(points: ReturnType<typeof point>[], tasks = [{ id: taskId }]) {
  return spawnSync("sh", ["-c", `${assertion}\nassert_latency_history "$1"`, "smoke-test", taskId], {
    input: JSON.stringify({ tasks, points }), encoding: "utf8",
  });
}

describe("latency history smoke assertion", () => {
  test("accepts a single averaged bucket with floating point rounding", () => {
    const result = check([point(120, 38.400000000000006, 50)]);
    expect(result.status).toBe(0);
    expect(result.stderr).toBe("");
  });

  test("accepts samples split across a minute boundary in either response order", () => {
    const points = [point(120, 28.4, 25), point(180, 48.4, 75)];
    expect(check(points).status).toBe(0);
    expect(check([...points].reverse()).status).toBe(0);
  });

  test("rejects missing, corrupt, duplicate and unrelated samples", () => {
    for (const points of [
      [], [point(120, 28.4, 25)], [point(120, 38.4, 51)],
      [point(120, 38.4, 50, "other-task")],
      [point(120, 28.4, 25), point(240, 48.4, 75)],
      [point(120, 28.4, 25), point(180, 47, 75)],
      [point(120, 28.4, 25), point(180, 48.4, 76)],
      [point(120, 38.4, 50), point(120, 38.4, 50)],
    ]) {
      const result = check(points);
      expect(result.status).toBe(1);
      expect(result.stderr).toContain("latency history assertion failed");
      expect(result.stderr).toContain(JSON.stringify({ tasks: [{ id: taskId }], points }));
    }
  });

  test("requires the task to remain assigned", () => {
    expect(check([point(120, 38.4, 50)], []).status).toBe(1);
  });
});
