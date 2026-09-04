import { ArrowDown, ArrowUp, Database, ServerIcon, WalletCards } from "lucide-react";
import { formatBytes, formatCurrency, formatSpeedParts, isOnline, number, remainingAssetValue } from "../format";
import { assetCurrency, themeToggle } from "../theme";
import type { Config, ExchangeRates, Server } from "../types";
import { ui } from "../locale";

interface Stat {
  icon: typeof ServerIcon;
  label: string;
  value: string;
  unit?: string;
  detail: React.ReactNode;
  tone?: string;
  iconTone?: "success" | "info";
}

export function StatsBar({ servers, config, exchangeRates }: { servers: Server[]; config: Config; exchangeRates: ExchangeRates | null }) {
  if (!config.show_stats) return null;
  const displayCurrency = assetCurrency(config);
  const locale = config.locale;
  const showOnline = themeToggle(config, "showOnline");
  const onlineServers = servers.filter((server) => isOnline(server, config.offline_threshold_seconds));
  const trafficUp = servers.reduce((sum, server) => sum + number(server.net_tx_total), 0);
  const trafficDown = servers.reduce((sum, server) => sum + number(server.net_rx_total), 0);
  const speedUp = onlineServers.reduce((sum, server) => sum + number(server.net_out), 0);
  const speedDown = onlineServers.reduce((sum, server) => sum + number(server.net_in), 0);
  const formattedSpeedUp = formatSpeedParts(speedUp);
  const formattedSpeedDown = formatSpeedParts(speedDown);
  const peakUpload = onlineServers.reduce<Server | null>((peak, server) => !peak || number(server.net_out) > number(peak.net_out) ? server : peak, null);
  const peakDownload = onlineServers.reduce<Server | null>((peak, server) => !peak || number(server.net_in) > number(peak.net_in) ? server : peak, null);
  const peakDetail = (server: Server | null, field: "net_out" | "net_in") => {
    if (!server || number(server[field]) <= 0) return <span>{ui(locale, "暂无实时流量", "No live traffic")}</span>;
    return <span title={server.name}>{ui(locale, `峰值 ${server.name}`, `Peak ${server.name}`)}</span>;
  };
  const paid = servers.filter((server) => server.price > 0);
  const toDisplayCurrency = (value: number, currency: string) => {
    const code = currency.trim().toUpperCase();
    const sourceRate = code === "CNY" ? 1 : exchangeRates?.rates[code];
    const targetRate = displayCurrency === "CNY" ? 1 : exchangeRates?.rates[displayCurrency];
    if (typeof sourceRate !== "number" || !Number.isFinite(sourceRate) || sourceRate <= 0) return null;
    if (typeof targetRate !== "number" || !Number.isFinite(targetRate) || targetRate <= 0) return null;
    return value / sourceRate * targetRate;
  };
  const converted = paid.map((server) => ({ server, price: toDisplayCurrency(server.price, server.currency) }));
  const missingRates = converted.filter(({ price }) => price === null).length;
  const totalValue = converted.reduce((sum, { price }) => sum + (price ?? 0), 0);
  const remainingValue = converted.reduce((sum, { server, price }) => {
    if (price === null) return sum;
    const remaining = remainingAssetValue(server.price, server.billing_cycle, server.expires_at);
    return sum + remaining / server.price * price;
  }, 0);

  const stats: Stat[] = [];
  if (showOnline) stats.push({
      icon: ServerIcon,
      label: ui(locale, "在线节点", "Online nodes"),
      value: String(onlineServers.length),
      unit: ui(locale, `/ ${servers.length} 台`, `/ ${servers.length}`),
      tone: "success-text",
      detail: servers.length === onlineServers.length ? ui(locale, "全部运行正常", "All systems operational") : ui(locale, `${servers.length - onlineServers.length} 台离线`, `${servers.length - onlineServers.length} offline`),
    });

  if (config.show_assets) stats.push({
    icon: WalletCards,
    label: ui(locale, "资产", "Assets"),
    value: formatCurrency(totalValue, displayCurrency),
    detail: <><span>{ui(locale, `剩余价值 ${formatCurrency(remainingValue, displayCurrency)}`, `Remaining value ${formatCurrency(remainingValue, displayCurrency)}`)}</span>{missingRates ? <span>{ui(locale, `${missingRates} 台汇率缺失`, `${missingRates} missing rates`)}</span> : null}</>,
  });
  if (config.show_traffic) stats.push({
    icon: Database,
    label: ui(locale, "累计流量", "Total traffic"),
    value: formatBytes(trafficUp + trafficDown),
    detail: <><span className="success-text">{ui(locale, "上传", "Upload")} {formatBytes(trafficUp)}</span><span className="info-text">{ui(locale, "下载", "Download")} {formatBytes(trafficDown)}</span></>,
  });
  if (config.show_speed) stats.push({
    icon: ArrowUp,
    label: ui(locale, "实时上行", "Live upload"),
    value: formattedSpeedUp.value,
    unit: formattedSpeedUp.unit,
    detail: peakDetail(peakUpload, "net_out"),
    tone: "success-text",
    iconTone: "success",
  }, {
    icon: ArrowDown,
    label: ui(locale, "实时下行", "Live download"),
    value: formattedSpeedDown.value,
    unit: formattedSpeedDown.unit,
    detail: peakDetail(peakDownload, "net_in"),
    tone: "info-text",
    iconTone: "info",
  });

  if (!stats.length) return null;
  return (
    <section className={`overview glass-panel overview-${Math.min(stats.length, 5)}`} aria-label={ui(locale, "服务器总览", "Server overview")}>
      {stats.map(({ icon: Icon, label, value, unit, detail, tone, iconTone }) => (
        <div className="overview-item" key={label}>
          <div className="overview-label"><span>{label}</span><span className={`stat-icon ${iconTone ?? ""}`}><Icon size={17} /></span></div>
          <div className="overview-value"><strong className={tone}>{value}</strong>{unit ? <b>{unit}</b> : null}</div>
          <div className="overview-detail">{detail}</div>
        </div>
      ))}
    </section>
  );
}
