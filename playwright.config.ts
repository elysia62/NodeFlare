import { defineConfig, devices } from "@playwright/test";

// 视觉回归 + 端到端测试。
//
// 视觉基线需要稳定的数据源，因此截图用例走 demo 模式（frontend/src/demo.ts，
// 静态数据，不需要后端与 agent 上报）。真实数据流的用例走已部署的后端。
//
// Chromium 用系统包（/usr/bin/chromium），避免再下载一份 ~150MB 的浏览器。
const BASE_URL = process.env.NODEFLARE_BASE_URL ?? "http://localhost:5173";
const EXECUTABLE = process.env.CHROMIUM_PATH ?? "/usr/bin/chromium";

export default defineConfig({
  testDir: "./e2e",
  // 基线截图存两份：本地看细节，CI 用固定平台名保持一致
  snapshotPathTemplate: "{testDir}/__screenshots__/{testFilePath}/{arg}{ext}",
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: [["list"], ["html", { open: "never", outputFolder: "e2e-report" }]],
  timeout: 60_000,
  expect: {
    toHaveScreenshot: {
      // 状态圆点带 2s 无限 status-pulse 动画，截图会捕捉到不同相位。
      // Playwright 的 animations:"disabled" 不覆盖这类无限动画（实测仍有
      // 约 1800 像素抖动），因此在页面对象里用 CSS 强制停掉动画，
      // 冻结后差异为 0，可以把阈值收紧到能抓出真实的样式改动。
      threshold: 0.01,
      maxDiffPixelRatio: 0.0005,
      animations: "disabled",
      caret: "hide",
    },
  },
  use: {
    baseURL: BASE_URL,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 1,
    launchOptions: {
      executablePath: EXECUTABLE,
      args: ["--no-sandbox", "--disable-gpu", "--disable-dev-shm-usage", "--font-render-hinting=none"],
    },
  },
  projects: [
    {
      name: "chromium",
      // 只用 Desktop Chrome 的设备特性（UA、触屏等），viewport 由顶层 use 决定，
      // 否则这里展开会覆盖掉 1440x900，导致基线尺寸与断言时不一致。
      use: {
        userAgent: devices["Desktop Chrome"].userAgent,
        isMobile: false,
        hasTouch: false,
      },
    },
  ],
  outputDir: "e2e-results",
});
