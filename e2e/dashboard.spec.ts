import { expect, test } from "@playwright/test";
import { Dashboard } from "./dashboard";

/**
 * 端到端行为测试：跑在真实浏览器引擎上，覆盖单元测试（bun test + 手写 DOM mock）
 * 触及不到的部分 —— 真实事件循环里的监听器协作、WebSocket 重连、页面导航。
 *
 * 这些用例走 demo 模式，只依赖前端，不需要后端与 agent 上报。
 */

/** 切到别的标签页再切回来。 */
async function switchAwayAndBack(page: import("@playwright/test").Page, hiddenMs: number) {
  const setHidden = (hidden: boolean) =>
    page.evaluate((h) => {
      Object.defineProperty(document, "hidden", { value: h, configurable: true });
      Object.defineProperty(document, "visibilityState", {
        value: h ? "hidden" : "visible",
        configurable: true,
      });
      document.dispatchEvent(new Event("visibilitychange"));
    }, hidden);

  await setHidden(true);
  await page.waitForTimeout(hiddenMs);
  await setHidden(false);
}

test.describe("看板", () => {
  test("渲染节点卡片与关键指标", async ({ page }) => {
    const dash = new Dashboard(page);
    await dash.open({ demo: true });
    await expect(dash.cards.first()).toBeVisible();
    expect(await dash.cards.count()).toBeGreaterThan(1);

    const card = dash.card("香港 Edge 01");
    await expect(card).toBeVisible();
    await expect(card).toContainText("CPU");
    await expect(card).toContainText("内存");
    await expect(card).toContainText("硬盘");
  });

  test("可以打开节点详情并返回", async ({ page }) => {
    const dash = new Dashboard(page);
    await dash.open({ demo: true });
    await dash.openDetail("香港 Edge 01");
    await expect(page).toHaveURL(/\/instance\/hongkong-edge/);
    // 详情页有图表容器
    await expect(page.locator(".detail-chart").first()).toBeVisible();

    await page.goBack();
    await expect(dash.cards.first()).toBeVisible();
  });
});

test.describe("标签页切换", () => {
  // 回归 7ce7805：invalidate() 曾连 HTTP 快照证据一并清空，
  // 切回后 WS 虽已重连但尚无上报批次，卡片会闪一下「待刷新」。
  test("短暂切走后返回，不显示待刷新", async ({ page }) => {
    const dash = new Dashboard(page);
    await dash.open({ demo: true });
    const before = await dash.state("香港 Edge 01");
    expect(before.online).toBe(true);

    await switchAwayAndBack(page, 3000);

    const after = await dash.state("香港 Edge 01");
    expect(after.pending).toBe(false);
    expect(after.online).toBe(true);
  });

  test("切走后返回仍保持在线（多次切换）", async ({ page }) => {
    const dash = new Dashboard(page);
    await dash.open({ demo: true });
    for (let i = 0; i < 3; i++) {
      await switchAwayAndBack(page, 800);
      const state = await dash.state("香港 Edge 01");
      expect(state.online, `第 ${i + 1} 次切换后`).toBe(true);
    }
  });
});
