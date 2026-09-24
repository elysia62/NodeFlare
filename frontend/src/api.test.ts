import { describe, expect, spyOn, test } from "bun:test";
import { api, ApiError } from "./api";

describe("history throttling", () => {
  test("preserves Retry-After so cards can retry without waiting for a full refresh", async () => {
    const fetchMock = spyOn(globalThis, "fetch");
    const clock = spyOn(Date, "now").mockReturnValue(Date.UTC(2026, 8, 24));
    try {
      for (const [header, delay] of [
        ["3", 3_000],
        ["Thu, 24 Sep 2026 00:00:05 GMT", 5_000],
        ["0", 1_000],
        ["invalid", undefined],
        [null, undefined],
      ] as const) {
        fetchMock.mockResolvedValueOnce(new Response(JSON.stringify({ error: "请求过于频繁" }), {
          status: 429,
          headers: header === null ? {} : { "Retry-After": header },
        }));
        const error = await api.latencyHistory("node", 1).catch(reason => reason);
        expect(error).toBeInstanceOf(ApiError);
        expect(error.status).toBe(429);
        expect(error.retryAfterMs).toBe(delay);
      }
      // The API layer must not silently retry arbitrary operations.
      expect(fetchMock).toHaveBeenCalledTimes(5);
    } finally {
      fetchMock.mockRestore();
      clock.mockRestore();
    }
  });
});
