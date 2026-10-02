import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { Select } from "./Select";
import { trafficTimezones } from "./shared";

test("timezone control is select-only, with UTC displayed and an associated label", () => {
  const markup = renderToStaticMarkup(<Select label="流量重置时区" value="UTC" options={trafficTimezones("zh-CN")} onChange={() => {}} />);
  expect(markup).toContain('role="combobox"');
  expect(markup).toContain('aria-expanded="false"');
  expect(markup).toContain('type="button"');
  expect(markup).toContain('aria-labelledby=');
  expect(markup).toContain('title="UTC">UTC</span>');
  expect(markup).not.toContain("<input");
  expect(markup).not.toContain("<datalist");
});

test("timezone control displays the existing value without falling back to UTC", () => {
  const markup = renderToStaticMarkup(<Select label="流量重置时区" value="America/Toronto" options={trafficTimezones("zh-CN", "America/Toronto")} onChange={() => {}} />);
  expect(markup).toContain('title="America/Toronto">America/Toronto</span>');
});

test("multi-select displays all selected channels in option order", () => {
  const markup = renderToStaticMarkup(<Select multiple label="启用通知渠道" value={["discord", "telegram", "bark"]} placeholder="未启用通知渠道"
    options={[{ value: "telegram", label: "Telegram" }, { value: "bark", label: "Bark" }, { value: "discord", label: "Discord" }]} onChange={() => {}} />);
  expect(markup).toContain('title="Telegram / Bark / Discord">Telegram / Bark / Discord</span>');
  expect(markup).not.toContain("<input");
});

test("multi-select can show no enabled channels and remain disabled until loading completes", () => {
  const markup = renderToStaticMarkup(<Select multiple disabled label="启用通知渠道" value={[]} placeholder="未启用通知渠道"
    options={[{ value: "telegram", label: "Telegram" }]} onChange={() => {}} />);
  expect(markup).toContain('title="未启用通知渠道">未启用通知渠道</span>');
  expect(markup).toContain('disabled=""');
});
