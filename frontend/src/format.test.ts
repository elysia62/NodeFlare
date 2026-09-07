import { describe, expect, test } from "bun:test";
import { formatByteSize, formatExpire, formatPrice, formatSpeedParts, parseByteSize } from "./format";

describe("parseByteSize", () => {
  test("accepts manual units and defaults to GB", () => {
    expect(parseByteSize("100 G")).toBe(100 * 1024 ** 3);
    expect(parseByteSize("0.5T")).toBe(0.5 * 1024 ** 4);
    expect(parseByteSize("512 m")).toBe(512 * 1024 ** 2);
    expect(parseByteSize("2048 kb")).toBe(2048 * 1024);
    expect(parseByteSize("200")).toBe(200 * 1024 ** 3);
  });

  test("rejects malformed input", () => {
    expect(parseByteSize("100 X")).toBeNull();
    expect(parseByteSize("abc")).toBeNull();
    expect(parseByteSize("")).toBeNull();
    expect(parseByteSize("-5 G")).toBeNull();
  });
});

describe("formatByteSize", () => {
  test("round-trips through parseByteSize", () => {
    for (const bytes of [0, 512 * 1024 ** 2, 100 * 1024 ** 3, 2 * 1024 ** 4]) {
      expect(parseByteSize(formatByteSize(bytes))).toBe(bytes);
    }
  });

  test("normalises negatives to zero", () => {
    expect(formatByteSize(0)).toBe("0");
    expect(formatByteSize(-1)).toBe("0");
  });
});

describe("formatSpeedParts", () => {
  test("keeps the numeric value separate from its unit", () => {
    expect(formatSpeedParts(1.5 * 1024 ** 2)).toEqual({ value: "1.5", unit: "MB/s" });
    expect(formatSpeedParts(0)).toEqual({ value: "0", unit: "B/s" });
  });
});

describe("formatPrice", () => {
  test("renders zero prices as free", () => {
    expect(formatPrice({ price: 0, billing_cycle: 30, currency: "CNY" })).toBe("免费");
    expect(formatPrice({ price: 0, billing_cycle: 30, currency: "USD" }, "en")).toBe("Free");
  });

  test("preserves paid and one-time cycles", () => {
    expect(formatPrice({ price: 5, billing_cycle: 0, currency: "CNY" })).toBe("¥5 / 一次性");
    expect(formatPrice({ price: 5, billing_cycle: 365, currency: "CNY" })).toBe("¥5 / 年");
  });
});

describe("formatExpire", () => {
  test("respects the expiration date even when a node is free", () => {
    expect(formatExpire({ price: 0, expires_at: null })).toBe("长期");
    expect(formatExpire({ price: 0, expires_at: 1 })).toBe("已过期");
    expect(formatExpire({ price: 5, expires_at: null })).toBe("未设置");
  });
});
