import { expect, test } from "@playwright/test";
import { Dashboard } from "./dashboard";

/**
 * 视觉回归：整页截图基线。
 *
 * 走 demo 模式（静态数据），使截图不受后端上报节奏影响。
 * 基线首次运行生成，之后每次比对；界面改动需显式更新基线：
 *   bun run test:visual -- --update-snapshots
 */
test.describe("视觉基线", () => {
  test("桌面看板整页", async ({ page }) => {
    const dash = new Dashboard(page);
    await dash.open({ demo: true });
    await dash.settle();
    await expect(page).toHaveScreenshot("dashboard-desktop.png", { fullPage: true });
  });

  test("节点详情页", async ({ page }) => {
    const dash = new Dashboard(page);
    await dash.open({ demo: true });
    await dash.settle();
    await dash.openDetail("香港 Edge 01");
    await dash.settle();
    // recharts 每次布局的亚像素测量有微小差异（实测约 2000 像素抖动），
    // 图表区域用 mask 屏蔽，页面其余部分仍严格比对。
    await expect(page).toHaveScreenshot("instance-detail.png", {
      fullPage: true,
      mask: [page.locator(".detail-chart")],
    });
  });

  test("窄屏看板", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    const dash = new Dashboard(page);
    await dash.open({ demo: true });
    await dash.settle();
    await expect(page).toHaveScreenshot("dashboard-mobile.png", { fullPage: true });
  });
});
