# 端到端与视觉测试

用 Playwright 驱动真实 Chromium，覆盖 `bun test` 的单元测试（手写 DOM mock）触及不到的部分：

- 真实事件循环里的监听器协作与 WebSocket 重连时序
- 整页视觉回归（布局、字体、样式）

## 为什么需要它

单元测试用 `bun test` + 手写 DOM mock 覆盖纯逻辑，但 mock 出的行为就是假设的行为。典型案例是提交 `7ce7805` 修复的「切标签页后卡片短暂显示待刷新」：

三个 `visibilitychange` 监听器（`AppProvider.tsx` 的轮询与播放、`transport.ts` 的重连）靠注册顺序隐式协作，逐个调用函数会天然跳过这段时序，只有真实浏览器能复现。

## 环境准备

系统 Chromium（Debian 包，无需再下载 Playwright 自带的 ~150MB 浏览器）：

```bash
apt-get install -y chromium fonts-noto-cjk
```

`fonts-noto-cjk` 是必需的：界面全中文，缺字体会渲染成方块，视觉基线失去意义。

Playwright 通过 `playwright.config.ts` 里的 `executablePath` 指向 `/usr/bin/chromium`。

## 运行

```bash
# 视觉回归（demo 模式，只需前端 dev server）
bun run --cwd frontend dev            # 另开终端
bun run test:visual

# 全部 e2e（含真实数据用例，需要后端 + agent 上报）
bun run test:e2e

# 界面改动后更新基线
bun run test:visual:update
```

## 用例分组

| 文件 | 依赖 | 覆盖 |
| --- | --- | --- |
| `visual.spec.ts` | 仅前端 | 看板整页、详情页、窄屏三张截图基线 |
| `dashboard.spec.ts` | 仅前端 | 渲染、详情页导航、切标签页不闪待刷新 |
| `live.spec.ts` | 后端 + agent | 真实数据流下的切标签页行为 |

`live.spec.ts` 在后端不可用或没有节点时自动跳过。

## 为什么切标签页用例分两份

demo 模式的状态判定走的是另一条分支：

```ts
next[server.id] = demoMode
  ? isOnline(server, threshold, clockNow / 1000)   // 纯时间戳，永不返回 pending
  : nodeObservation.current.status(server, threshold, clockNow);  // 快照/流证据交互
```

因此 `dashboard.spec.ts` 里的切换用例只验证「UI 不崩」，**对 `7ce7805` 那类回归没有防护力**。真正起作用的是 `live.spec.ts`——已实测：临时还原到有 bug 的 `nodeStatus.ts` 后，该用例失败并报出「待刷新」。

## 视觉测试的两个坑

**1. 无限动画会污染截图。** `.status-dot` 有 2s 循环的 `status-pulse`，每次截图捕捉到不同相位，带来约 1800 像素抖动。Playwright 的 `animations: "disabled"` 不覆盖无限动画，需在页面对象里自行注入 CSS 停掉（见 `dashboard.ts` 的 `settle()`）。冻结后差异为 0。

**2. recharts 图表有亚像素抖动。** 详情页图表每次布局约有 2000 像素差异，用 `mask` 屏蔽 `.detail-chart`，页面其余部分仍严格比对。

由于噪声已降到 0，阈值可以设得很紧（`maxDiffPixelRatio: 0.0005`），从而能抓出真实改动。已验证的变异测试：

| 改动 | 结果 |
| --- | --- |
| 卡片圆角 12px → 0px | ✅ 被抓住 |
| 详情页内边距 14/20px → 40/60px | ✅ 被抓住 |
| 详情页圆角 8px → 0px | ⚪ 未抓住（仅 103 像素，实际不可见） |

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `NODEFLARE_BASE_URL` | `http://localhost:5173` | 前端地址（视觉与 demo 用例） |
| `NODEFLARE_LIVE_URL` | `http://127.0.0.1:2206` | 后端地址（真实数据用例） |
| `CHROMIUM_PATH` | `/usr/bin/chromium` | Chromium 可执行文件 |

## 注意

- **Vite 默认监听 IPv6**（`[::1]:5173`），用 `localhost` 而非 `127.0.0.1` 访问。
- 截图基线（`e2e/__screenshots__/`）需提交；`e2e-report/`、`e2e-results/` 已加入 `.gitignore`。
- 视觉基线对渲染环境敏感（字体、系统库），跨机器比对可能失败。这类用例更适合作为本地与固定 CI 环境的回归手段，而不是跨环境的一致性检查。
