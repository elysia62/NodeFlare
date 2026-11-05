import { afterEach, beforeEach, expect, setSystemTime, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { demoConfig, demoExchangeRates, demoServers } from "../demo";
import { NodeCard } from "./NodeCard";
import { StatsBar } from "./StatsBar";

const now = Date.UTC(2026, 10, 6) / 1000;
const prepaid = { ...demoServers[0], price: 30, currency: "CNY", billing_cycle: 30,
  expires_at: now + 90 * 86400, timestamp: now };
const config = { ...demoConfig, show_stats: true, show_assets: true, show_price: true,
  show_expiry: true, show_latency: false, theme_options: { assetCurrency: "CNY" } };

beforeEach(() => setSystemTime(new Date(now * 1000)));
afterEach(() => setSystemTime());

test("card and overview show all prepaid service even when it exceeds the period price", () => {
  const card = renderToStaticMarkup(<NodeCard server={prepaid} status="online" config={config} liveConnected onOpen={() => {}} />);
  const overview = renderToStaticMarkup(<StatsBar servers={[prepaid]} statuses={{ [prepaid.id]: "online" }} config={config} exchangeRates={null} />);
  expect(card).toContain("<em>¥90</em>");
  expect(overview).toContain("剩余价值 ¥90");
  expect(overview).toContain("<strong>¥30</strong>");
});

test("overview converts the full remaining value and reports excluded currencies", () => {
  const usd = { ...prepaid, id: "usd", price: 3, currency: "USD" };
  const eur = { ...prepaid, id: "eur", price: 10, currency: "EUR" };
  const rates = { ...demoExchangeRates, rates: { CNY: 1, USD: 0.1 } };
  const markup = renderToStaticMarkup(<StatsBar servers={[prepaid, usd, eur]} statuses={{}} config={config} exchangeRates={rates} />);
  expect(markup).toContain("剩余价值 ¥180");
  expect(markup).toContain("1 台汇率缺失");
});
