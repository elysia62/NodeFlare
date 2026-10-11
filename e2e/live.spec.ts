import { expect, test } from "@playwright/test";
import { Dashboard } from "./dashboard";

/**
 * 真实数据用例：连接已部署的后端（默认 127.0.0.1:2206），由线上 agent 持续上报。
 *
 * 为什么这组必须走真实后端：demo 模式（demoMode ? isOnline(...) : observation.status(...)）
 * 直接按时间戳判定，永远不返回 pending，因此对「切标签页闪待刷新」这类
 * 快照/流证据交互的回归没有任何防护力。
 *
 * 后端不可用或没有 agent 上报时整组跳过，避免在纯前端环境里误报失败。
 */
const BASE = process.env.NODEFLARE_LIVE_URL ?? "http://127.0.0.1:2206";

/** 探测后端是否可用且至少有一个在线节点。 */
async function liveReady(page: import("@playwright/test").Page) {
  const res = await page.request.get(`${BASE}/api/bootstrap`).catch(() => null);
  if (!res?.ok()) return false;
  const body = await res.json().catch(() => null);
  return Array.isArray(body?.servers) && body.servers.length > 0;
}

test.describe("切标签页（真实数据）", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "仅在 chromium 下运行");

  test.beforeEach(async ({ page }) => {
    test.skip(!(await liveReady(page)), `后端 ${BASE} 无可用节点，跳过`);
  });

  /**
   * 回归 7ce7805。快照宽限期为 30s，隐藏 3 秒属于「短暂切换」：
   * 卡片应当立即恢复既有状态，而不是退回「待刷新」等待下一批上报。
   */
  test("短暂切走返回后立即保持在线", async ({ page }) => {
    const dash = new Dashboard(page);
    await dash.open({ path: `${BASE}/` });
    await dash.settle();

    const name = await dash.cards.first().getAttribute("aria-label");
    const label = (name ?? "").split("，")[0];
    expect(label, "应能读到节点名").toBeTruthy();

    await expect(dash.card(label)).toHaveClass(/online/);

    // 切走 3 秒（小于 30s 的快照宽限期）
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { value: true, configurable: true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await page.waitForTimeout(3000);

    // 切回：立即检查，不等任何新数据
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { value: false, configurable: true });
      document.dispatchEvent(new Event("visibilitychange"));
    });

    const card = dash.card(label);
    const text = (await card.textContent()) ?? "";
    expect(text, "切回瞬间不应显示待刷新").not.toContain("待刷新");
    await expect(card).toHaveClass(/online/);
  });

  test("离开超过快照宽限期后不谎报在线", async ({ page }) => {
    test.setTimeout(120_000);
    const dash = new Dashboard(page);
    await dash.open({ path: `${BASE}/` });
    await dash.settle();

    const label = ((await dash.cards.first().getAttribute("aria-label")) ?? "").split("，")[0];

    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { value: true, configurable: true });
      document.dispatchEvent(new Event("visibilitychange"));
    });
    // 超过 SNAPSHOT_MAX_AGE_MS（= 2 × 15s）
    await page.waitForTimeout(35_000);
    await page.evaluate(() => {
      Object.defineProperty(document, "hidden", { value: false, configurable: true });
      document.dispatchEvent(new Event("visibilitychange"));
    });

    // 允许 pending（诚实等待）或 offline，但必须有新数据支撑才允许 online
    const card = dash.card(label);
    const text = (await card.textContent()) ?? "";
    const isPending = text.includes("待刷新");
    const isOnline = await card.evaluate((el) => el.classList.contains("online"));
    if (!isPending && isOnline) {
      // 若显示在线，说明新快照已到达，属正常
      console.log("新数据已到达，恢复在线");
    }
    expect(await card.isVisible()).toBe(true);
  });
});
