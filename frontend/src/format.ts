import type { Server, TrafficLimitType } from "./types";
import { ui, type UiLocale } from "./locale";

export function number(value: number | null | undefined) {
  return Number.isFinite(value) ? Number(value) : 0;
}

export function percent(used: number | null, total: number | null) {
  const safeTotal = number(total);
  return safeTotal > 0 ? Math.min(100, Math.max(0, (number(used) / safeTotal) * 100)) : 0;
}

interface FormattedBytes {
  value: string;
  unit: string;
}

function formatBytesParts(value: number | null | undefined, decimals = 1): FormattedBytes {
  const size = Math.max(0, number(value));
  if (size === 0) return { value: "0", unit: "B" };
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(units.length - 1, Math.floor(Math.log(size) / Math.log(1024)));
  return {
    value: (size / 1024 ** index).toFixed(index === 0 ? 0 : decimals),
    unit: units[index],
  };
}

export function formatBytes(value: number | null | undefined, decimals = 1) {
  const formatted = formatBytesParts(value, decimals);
  return `${formatted.value} ${formatted.unit}`;
}

export function formatSpeed(value: number | null | undefined) {
  return `${formatBytes(value)}/s`;
}

export function formatSpeedParts(value: number | null | undefined): FormattedBytes {
  const formatted = formatBytesParts(value);
  return { ...formatted, unit: `${formatted.unit}/s` };
}

const BYTE_UNIT_FACTORS: Record<string, number> = {
  "": 1024 ** 3,
  b: 1,
  k: 1024, kb: 1024, kib: 1024,
  m: 1024 ** 2, mb: 1024 ** 2, mib: 1024 ** 2,
  g: 1024 ** 3, gb: 1024 ** 3, gib: 1024 ** 3,
  t: 1024 ** 4, tb: 1024 ** 4, tib: 1024 ** 4,
};

export function parseByteSize(text: string): number | null {
  const match = /^(\d+(?:\.\d+)?)\s*([a-zA-Z]*)$/.exec(text.trim());
  if (!match) return null;
  const factor = BYTE_UNIT_FACTORS[match[2].toLowerCase()];
  return factor === undefined ? null : Math.round(Number(match[1]) * factor);
}

export function formatByteSize(bytes: number): string {
  const size = Math.max(0, Math.round(number(bytes)));
  if (size === 0) return "0";
  for (const [suffix, factor] of [["T", 1024 ** 4], ["G", 1024 ** 3], ["M", 1024 ** 2], ["K", 1024]] as const) {
    if (size >= factor) return `${Math.round((size / factor) * 100) / 100} ${suffix}`;
  }
  return `${size} B`;
}

export function formatCpuName(model: string | null | undefined, cores: number | null | undefined) {
  const name = (model ?? "").replace(/\s+/g, " ").trim() || "--";
  const count = Math.max(0, Math.trunc(number(cores)));
  return count > 0 ? `${name} ×${count}` : name;
}

const GPU_MODEL_HINT = /(graphics|geforce|radeon|\bgpu\b|\buhd\b|\biris\b|\barc\b|quadro|tesla|\brtx\b|\bgtx\b|apple|adreno|mali|videocore)/i;
const GPU_DEVICE_HINT = new RegExp(`${GPU_MODEL_HINT.source}|nvidia|amd|intel`, "i");
const GPU_REJECT = /(sensor hub|management engine|ethernet|wireless|audio controller|usb controller|sata controller)/i;

function conciseGpuName(raw: string | null | undefined) {
  const original = (raw ?? "").replace(/\s+/g, " ").trim();
  if (!original || /^(?:-|0|none|null|unknown|n\/a)$/i.test(original) || GPU_REJECT.test(original)) return "";

  const bracketModels = Array.from(original.matchAll(/\[([^\]]+)]/g), (match) => match[1].trim());
  const bracketModel = [...bracketModels].reverse().find((part) => GPU_MODEL_HINT.test(part));
  let model = bracketModel ?? original;
  if (!GPU_DEVICE_HINT.test(model)) return "";

  model = model
    .replace(/\bIntel Corporation\b/gi, "Intel")
    .replace(/\bNVIDIA Corporation\b/gi, "NVIDIA")
    .replace(/\bAdvanced Micro Devices,?\s*Inc\.?/gi, "AMD")
    .replace(/\bAMD\/ATI\b/gi, "AMD")
    .replace(/\s*\[(?:AMD|ATI|AMD\/ATI)]\s*/gi, " ")
    .replace(/\s+/g, " ")
    .trim();

  if (bracketModel) {
    if (/Intel/i.test(original) && !/^Intel\b/i.test(model)) model = `Intel ${model}`;
    else if (/NVIDIA/i.test(original) && !/^NVIDIA\b/i.test(model)) model = `NVIDIA ${model}`;
    else if (/(AMD|Advanced Micro Devices|ATI)/i.test(original) && !/^AMD\b/i.test(model)) model = `AMD ${model}`;
  }
  return model;
}

export function displayGpuDevices(gpus: Server["gpus"]) {
  const seen = new Set<string>();
  return gpus.flatMap((gpu) => {
    const name = conciseGpuName(gpu.model);
    if (!name || seen.has(name)) return [];
    seen.add(name);
    return [{ ...gpu, model: name }];
  });
}

export function formatUptime(seconds: number | null | undefined, locale: UiLocale = "zh-CN") {
  const days = Math.floor(number(seconds) / 86400);
  if (days > 0) return ui(locale, `${days} 天`, `${days} days`);
  const hours = Math.floor(number(seconds) / 3600);
  if (hours > 0) return ui(locale, `${hours} 小时`, `${hours} hours`);
  const minutes = Math.floor(number(seconds) / 60);
  return ui(locale, `${minutes} 分钟`, `${minutes} minutes`);
}

export function isOnline(server: Pick<Server, "timestamp">, threshold: number, at = Date.now() / 1000) {
  return !!server.timestamp && at - server.timestamp <= threshold;
}

export function countryFlag(region: string) {
  const code = region.trim().slice(0, 2).toUpperCase();
  if (!/^[A-Z]{2}$/.test(code)) return "";
  return String.fromCodePoint(...[...code].map((char) => 127397 + char.charCodeAt(0)));
}

export function trafficUsed(server: Pick<Server, "net_rx_total" | "net_tx_total" | "traffic_limit_type">) {
  const down = number(server.net_rx_total);
  const up = number(server.net_tx_total);
  const type: TrafficLimitType = server.traffic_limit_type;
  if (type === "max") return Math.max(up, down);
  if (type === "min") return Math.min(up, down);
  if (type === "up") return up;
  if (type === "down") return down;
  return up + down;
}

const currencySymbols: Record<string, string> = {
  CNY: "¥", USD: "$", CAD: "CA$", HKD: "HK$", EUR: "€", GBP: "£", JPY: "¥",
  RUB: "₽", CHF: "CHF ", INR: "₹", VND: "₫", THB: "฿",
};

export function formatCurrency(value: number, currency = "CNY") {
  const code = currency.toUpperCase();
  const symbol = currencySymbols[code] ?? `${code} `;
  const rounded = (Math.round(number(value) * 100) / 100).toFixed(2).replace(/\.?0+$/, "");
  return `${symbol}${rounded || "0"}`;
}

export function formatPrice(server: Pick<Server, "price" | "billing_cycle" | "currency">, locale: UiLocale = "zh-CN") {
  if (server.price === 0) return ui(locale, "免费", "Free");
  const cycle = server.billing_cycle <= 0 ? ui(locale, "一次性", "one-time")
    : server.billing_cycle >= 27 && server.billing_cycle <= 32 ? ui(locale, "月", "month")
    : server.billing_cycle >= 87 && server.billing_cycle <= 95 ? ui(locale, "季", "quarter")
      : server.billing_cycle >= 175 && server.billing_cycle <= 185 ? ui(locale, "半年", "half-year")
        : server.billing_cycle >= 360 && server.billing_cycle <= 370 ? ui(locale, "年", "year")
          : ui(locale, `${server.billing_cycle} 天`, `${server.billing_cycle} days`);
  return `${formatCurrency(server.price, server.currency)} / ${cycle}`;
}

function daysUntil(timestamp: number | null) {
  if (!timestamp) return null;
  return Math.ceil((timestamp * 1000 - Date.now()) / 86_400_000);
}

export function formatExpire(server: Pick<Server, "expires_at" | "price">, locale: UiLocale = "zh-CN") {
  if (server.price === 0 && !server.expires_at) return ui(locale, "长期", "Lifetime");
  const days = daysUntil(server.expires_at);
  if (days === null) return ui(locale, "未设置", "Not set");
  if (days < 0) return ui(locale, "已过期", "Expired");
  return ui(locale, `${days} 天`, `${days} days`);
}

export function remainingAssetValue(price: number, billingCycle: number, expiresAt: number | null) {
  if (price <= 0) return 0;
  const days = daysUntil(expiresAt);
  if (days === null || days <= 0) return 0;
  if (billingCycle <= 0) return price;
  return price * Math.min(days / billingCycle, 1);
}
