import { describe, expect, test } from "bun:test";
import { reconnectDelay } from "./transport";

describe("reconnectDelay", () => {
  test("backs off exponentially", () => {
    expect(reconnectDelay(0, 0.5)).toBe(1_000);
    expect(reconnectDelay(1, 0.5)).toBe(2_000);
    expect(reconnectDelay(4, 0.5)).toBe(16_000);
  });

  test("adds bounded jitter and caps long retries", () => {
    expect(reconnectDelay(0, 0)).toBe(800);
    expect(reconnectDelay(0, 1)).toBe(1_200);
    expect(reconnectDelay(20, 1)).toBe(30_000);
  });

  test("normalizes invalid attempt and random inputs", () => {
    expect(reconnectDelay(-5, -1)).toBe(800);
    expect(reconnectDelay(0.9, 2)).toBe(1_200);
  });
});
