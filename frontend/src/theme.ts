import type { CarrierSlotKey } from "./latency";
import { ASSET_CURRENCIES, type AssetCurrency, type Config, type ThemeSettingValue } from "./types";

function option(config: Config, key: string): ThemeSettingValue | undefined {
  return config.theme_options?.[key];
}

export function assetCurrency(config: Config): AssetCurrency {
  const value = option(config, "assetCurrency");
  return typeof value === "string" && (ASSET_CURRENCIES as readonly string[]).includes(value)
    ? value as AssetCurrency
    : "CNY";
}

export function themeToggle(config: Config, key: string, fallback = true) {
  const value = option(config, key);
  return typeof value === "boolean" ? value : fallback;
}

function themeText(config: Config, key: string) {
  const value = option(config, key);
  return typeof value === "string" ? value.trim() : "";
}

/**
 * Carrier line names as configured in the theme settings. Blank entries are
 * intentional: the latency layer falls back to name matching when all three
 * are empty, so an unconfigured site still shows sensible lines.
 */
export function carrierSelection(config: Config): CarrierSlotKey {
  return {
    telecom: themeText(config, "telecomLatencyTask"),
    mobile: themeText(config, "mobileLatencyTask"),
    unicom: themeText(config, "unicomLatencyTask"),
  };
}

export function resolveBackground(raw: string, dark: boolean) {
  const parts = raw.split("|").map((part) => part.trim());
  if (!parts[0] && !parts[1]) return "";
  return dark ? parts[1] || parts[0] : parts[0];
}
