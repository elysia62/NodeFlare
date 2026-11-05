import { afterEach, beforeEach, describe, expect, setSystemTime, test } from "bun:test";
import { formatByteSize, formatExpire, formatLastSeen, formatPrice, formatSpeedParts, parseByteSize, remainingAssetValue } from "./format";

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

describe("remainingAssetValue", () => {
  const now = Date.UTC(2026, 9, 2) / 1000;
  const expiresIn = (days: number) => now + days * 86400;

  beforeEach(() => setSystemTime(new Date(now * 1000)));
  afterEach(() => setSystemTime());

  test("counts all prepaid periods instead of capping at one period", () => {
    expect(remainingAssetValue(30, 30, expiresIn(90))).toBe(90);
    expect(remainingAssetValue(36, 30, expiresIn(3254))).toBeCloseTo(3904.8, 8);
    expect(remainingAssetValue(36, 30, expiresIn(3650))).toBeCloseTo(4380, 8);
    expect(remainingAssetValue(120, 365, expiresIn(730))).toBeCloseTo(240, 8);
  });

  test("prorates within a period and supports multi-year annual billing", () => {
    expect(remainingAssetValue(36, 30, expiresIn(15))).toBeCloseTo(18, 8);
    expect(remainingAssetValue(36, 30, expiresIn(45))).toBeCloseTo(54, 8);
    expect(remainingAssetValue(120, 365, expiresIn(100))).toBeCloseTo(120 * 100 / 365, 8);
  });

  test("rounds future partial days up and expires at the exact deadline", () => {
    expect(remainingAssetValue(30, 30, now + 1)).toBe(1);
    expect(remainingAssetValue(30, 30, expiresIn(30) + 1)).toBeCloseTo(31, 8);
    expect(remainingAssetValue(30, 30, now)).toBe(0);
    expect(remainingAssetValue(30, 30, now - 1)).toBe(0);
    expect(formatExpire({ price: 30, expires_at: now })).toBe("已过期");
    expect(formatExpire({ price: 30, expires_at: now - 1 }, "en")).toBe("Expired");
  });

  test("uses the same long-term convention for the amount and expiry label", () => {
    expect(remainingAssetValue(30, 30, expiresIn(36500))).toBe(36500);
    expect(remainingAssetValue(30, 30, expiresIn(36501))).toBe(30);
    expect(formatExpire({ price: 30, expires_at: expiresIn(36501) })).toBe("长期");
  });

  test("ignores invalid prices and expiry dates", () => {
    expect(remainingAssetValue(Number.NaN, 30, expiresIn(30))).toBe(0);
    expect(remainingAssetValue(Number.POSITIVE_INFINITY, 30, expiresIn(30))).toBe(0);
    expect(remainingAssetValue(30, 30, Number.NaN)).toBe(0);
    expect(remainingAssetValue(30, 30, Number.POSITIVE_INFINITY)).toBe(0);
  });

  test("keeps one-time charges fixed and excludes expired or unpriced terms", () => {
    expect(remainingAssetValue(36, 0, expiresIn(3650))).toBe(36);
    expect(remainingAssetValue(36, 30, now)).toBe(0);
    expect(remainingAssetValue(36, 30, expiresIn(-1))).toBe(0);
    expect(remainingAssetValue(36, 30, null)).toBe(0);
    expect(remainingAssetValue(36, 0, expiresIn(-1))).toBe(0);
    expect(remainingAssetValue(0, 30, expiresIn(3650))).toBe(0);
  });
});

describe("formatLastSeen", () => {
  const at = 1_700_000_000;
  test("无上报时间时说明没有数据", () => {
    expect(formatLastSeen(0, "zh-CN", at)).toBe("暂无上报");
    expect(formatLastSeen(null, "zh-CN", at)).toBe("暂无上报");
  });
  test("按量级切换单位", () => {
    expect(formatLastSeen(at - 30, "zh-CN", at)).toBe("30 秒前");
    expect(formatLastSeen(at - 90, "zh-CN", at)).toBe("1 分钟前");
    expect(formatLastSeen(at - 7200, "zh-CN", at)).toBe("2 小时前");
    expect(formatLastSeen(at - 86400 * 3, "zh-CN", at)).toBe("3 天前");
  });
  test("未来时间不会出现负数", () => {
    expect(formatLastSeen(at + 500, "zh-CN", at)).toBe("0 秒前");
  });
});
