import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import { Dashboard } from "./dashboard";

/**
 * 视觉回归（真实代码路径）。
 *
 * visual.spec.ts 用 demo 模式，走的是 `demoMode ? isOnline(...) : observation.status(...)`
 * 里的前一个分支，只覆盖布局与样式。这里改走生产构建并拦截网络：
 * 页面照常执行真实的 bootstrap 与 WebSocket 更新链路，只是数据由 fixture
 * 提供，从而既可重复比对像素，又覆盖真实数据路径下的外观
 * （待刷新/离线状态、延迟面板的空态、快照与流证据的交互）。
 *
 * fixture 由真实后端录制（见 e2e/fixtures/README.md）。
 */

const here = __dirname;
const fixture = (name: string) =>
  JSON.parse(readFileSync(join(here, "fixtures", name), "utf8"));

/**
 * fixture 录制于某个过去时刻，直接用会因超出 offline_threshold 被判离线。
 * 这里把节点时间戳平移到现在，仅保留相对关系，让两种场景都有确定结果。
 */
function anchoredBootstrap(ageSeconds = 5) {
  const data = fixture("bootstrap.json");
  const now = Math.floor(Date.now() / 1000);
  // 原始数据里各节点的时间偏移是相对录制时刻的，整体平移保持相对关系
  const recordedAt = Math.max(...data.servers.map((s: { timestamp: number }) => s.timestamp));
  const shift = now - ageSeconds - recordedAt;
  for (const server of data.servers) {
    server.timestamp += shift;
  }
  return data;
}

/** 由 fixture 派生的一个批次更新，时间戳锚定在固定的过去时刻。 */
function batchUpdate() {
  const now = Math.floor(Date.now() / 1000);
  const server = anchoredBootstrap().servers[0];
  return {
    type: "batchUpdate",
    updates: [
      {
        server_id: server.id,
        timestamp: now,
        metrics: {
          cpu: 12.5,
          load1: 0.4,
          load5: 0.5,
          load15: 0.6,
          mem_used: 1_073_741_824,
          mem_total: 8_589_934_592,
          disk_used: 7_516_192_768,
          disk_total: 128_849_018_880,
          net_in: 4096,
          net_out: 2048,
          uptime: 172_800,
        },
        info: {},
      },
    ],
  };
}

/** 用 fixture 替换后端数据，使页面走真实（非 demo）分支。 */
async function stubBackend(page: Page, bootstrap = anchoredBootstrap()) {
  await page.route("**/api/bootstrap*", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(bootstrap) }),
  );
  await page.routeWebSocket(/\/api\/ws/, (ws) => {
    const server = ws.connectToServer();
    server.onMessage(() => {});
    ws.send(gzipSync(JSON.stringify(batchUpdate())));
  });
}

test.describe("真实数据路径视觉", () => {
  test("在线节点卡片", async ({ page }) => {
    await stubBackend(page);
    const dash = new Dashboard(page);
    await dash.open({ path: "/" });
    await dash.settle();
    await expect(page).toHaveScreenshot("real-dashboard.png", { fullPage: true });
  });

  test("待刷新状态（喂入陈旧快照）", async ({ page }) => {
    // 待刷新的条件是「没有可用的流证据，且快照已过期」。只要 bootstrap 还在
    // 正常轮询，snapshotAt 就会被不断刷新，所以让响应带上已超出离线阈值的
    // 时间戳，等价于真实环境中「agent 停止上报」时卡片的样子。
    const stale = anchoredBootstrap();
    const staleAt = Math.floor(Date.now() / 1000) - 3600;
    for (const server of stale.servers) {
      server.timestamp = staleAt;
    }
    await page.route("**/api/bootstrap*", (route) =>
      route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(stale) }),
    );
    await page.routeWebSocket(/\/api\/ws/, (ws) => {
      const server = ws.connectToServer();
      server.onMessage(() => {});
      // 不发任何帧：没有流证据，页面只能依赖快照
    });

    const dash = new Dashboard(page);
    await dash.open({ path: "/" });
    await page.waitForTimeout(3000);
    await dash.settle();
    await expect(page).toHaveScreenshot("real-dashboard-stale.png", { fullPage: true });
  });
});
