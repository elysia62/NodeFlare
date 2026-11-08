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
  expect(overview).toContain("剩余价值");
  expect(overview).toContain("<strong>¥90</strong>");
  expect(overview).toContain("每月成本 ¥30");
  expect(overview).not.toContain(">资产<");
});

test("overview converts the full remaining value and reports excluded currencies", () => {
  const usd = { ...prepaid, id: "usd", price: 3, currency: "USD" };
  const eur = { ...prepaid, id: "eur", price: 10, currency: "EUR" };
  const rates = { ...demoExchangeRates, rates: { CNY: 1, USD: 0.1 } };
  const markup = renderToStaticMarkup(<StatsBar servers={[prepaid, usd, eur]} statuses={{}} config={config} exchangeRates={rates} />);
  expect(markup).toContain("<strong>¥180</strong>");
  expect(markup).toContain("每月成本 ¥60");
  expect(markup).toContain("1 台汇率缺失");
});

test("same-currency assets remain visible without exchange rates", () => {
  const usd = { ...prepaid, currency: "USD" };
  const display = { ...config, theme_options: { assetCurrency: "USD" } };
  for (const exchangeRates of [null, { ...demoExchangeRates, rates: { CNY: 1 } }]) {
    const markup = renderToStaticMarkup(<StatsBar servers={[usd]} statuses={{}} config={display} exchangeRates={exchangeRates} />);
    expect(markup).toContain("<strong>$90</strong>");
    expect(markup).toContain("每月成本 $30");
    expect(markup).not.toContain("汇率缺失");
  }
});

test("monthly cost normalizes mixed billing cycles and excludes one-time charges", () => {
  const servers = [
    prepaid,
    { ...prepaid, id: "quarterly", price: 90, billing_cycle: 90 },
    { ...prepaid, id: "yearly", price: 36.5, currency: "USD", billing_cycle: 365 },
    { ...prepaid, id: "weekly", price: 7, billing_cycle: 7 },
    { ...prepaid, id: "one-time", price: 999, billing_cycle: 0 },
    { ...prepaid, id: "free", price: 0 },
  ];
  const rates = { ...demoExchangeRates, rates: { CNY: 1, USD: 0.1 } };
  const markup = renderToStaticMarkup(<StatsBar servers={servers} statuses={{}} config={config} exchangeRates={rates} />);
  expect(markup).toContain("每月成本 ¥120");
  expect(markup).toContain("<strong>¥1359</strong>");
});

test("one-time purchases retain remaining value without recurring monthly cost", () => {
  const purchase = { ...prepaid, price: 99, billing_cycle: 0 };
  const markup = renderToStaticMarkup(<StatsBar servers={[purchase]} statuses={{}} config={{ ...config, locale: "en" }} exchangeRates={null} />);
  expect(markup).toContain("Monthly cost ¥0");
  expect(markup).toContain("<strong>¥99</strong>");
});
