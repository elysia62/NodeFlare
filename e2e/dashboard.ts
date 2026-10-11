import { expect, type Page } from "@playwright/test";

/**
 * 看板页面对象。覆盖 demo 模式（静态数据）与真实后端两种运行方式，
 * 断言只依赖语义化的类名与可见文本，不绑定具体节点数量。
 */
export class Dashboard {
  constructor(private readonly page: Page) {}

  /** 打开看板；demo=true 时使用前端的静态演示数据，不依赖后端上报。 */
  async open({ demo = false, path = "/" }: { demo?: boolean; path?: string } = {}) {
    const url = demo ? `${path}${path.includes("?") ? "&" : "?"}demo=1` : path;
    await this.page.goto(url, { waitUntil: "domcontentloaded" });
    await this.cards.first().waitFor({ timeout: 45_000 });
    return this;
  }

  get cards() {
    return this.page.locator(".node-card");
  }

  card(name: string) {
    return this.page.locator(".node-card", { hasText: name });
  }

  /** 等待首屏动态数值稳定后再截图，避免数据分批到达造成抖动。 */
  async settle() {
    await this.page.waitForTimeout(2500);
    await this.page.evaluate(() => document.fonts.ready);
    // 状态圆点的 status-pulse 是无限动画，Playwright 的
    // animations:"disabled" 管不到它，只能自己停掉，否则每次截图
    // 都会捕捉到不同相位，给视觉比对引入约 1800 像素的噪声。
    await this.page.addStyleTag({
      content: `*, *::before, *::after { animation: none !important; transition: none !important; }`,
    });
    await this.page.waitForTimeout(200);
    return this;
  }

  async screenshot(name: string) {
    await expect(this.page).toHaveScreenshot(name, { fullPage: true });
  }

  async openDetail(name: string) {
    await this.card(name).click();
    await this.page.waitForURL(/\/instance\//, { timeout: 15_000 });
    return this;
  }

  /** 读取卡片当前状态，用于行为断言（不涉及像素）。 */
  async state(name: string) {
    const card = this.card(name);
    const cls = (await card.getAttribute("class")) ?? "";
    const text = (await card.textContent()) ?? "";
    return {
      online: cls.includes("online"),
      pending: text.includes("待刷新"),
    };
  }
}
