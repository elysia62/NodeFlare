import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { demoConfig, demoServers } from "../demo";
import { NodeCard } from "./NodeCard";
import { NodeDetails } from "./NodeDetails";
import { StatsBar } from "./StatsBar";

const server = { ...demoServers[0], timestamp: 1 };
const config = { ...demoConfig, show_latency: false, show_stats: true, show_speed: true,
  theme_options: { ...demoConfig.theme_options, showOnline: true } };

test("a pending card does not render a false offline overlay for an old timestamp", () => {
  const markup = renderToStaticMarkup(<NodeCard server={server} status="pending" config={config} liveConnected={false} onOpen={() => {}} />);
  expect(markup).toContain("待刷新");
  expect(markup).toContain('class="status-dot pending"');
  expect(markup).not.toContain("node-offline-overlay");
  expect(markup).not.toContain("离线");
});

test("confirmed offline cards still display the outage overlay", () => {
  const markup = renderToStaticMarkup(<NodeCard server={server} status="offline" config={config} liveConnected onOpen={() => {}} />);
  expect(markup).toContain("node-offline-overlay");
  expect(markup).toContain("离线");
});

test("overview distinguishes pending nodes and traffic from zero online nodes", () => {
  const markup = renderToStaticMarkup(<StatsBar servers={[server]} statuses={{ [server.id]: "pending" }} config={config} exchangeRates={null} />);
  expect(markup).toContain("1 台待刷新");
  expect(markup).toContain("—");
  expect(markup).not.toContain("1 台离线");
  expect(markup).not.toContain("全部运行正常");
});

test("detail status uses the same pending observation as cards and overview", () => {
  const markup = renderToStaticMarkup(<NodeDetails server={server} status="pending" retentionDays={30} locale="zh-CN" onClose={() => {}} />);
  expect(markup).toContain('class="status-chip pending"');
  expect(markup).toContain("待刷新");
  expect(markup).not.toContain("离线");
});
