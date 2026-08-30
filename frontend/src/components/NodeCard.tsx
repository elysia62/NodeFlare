import {
  CalendarDays,
  ChevronDown,
  ChevronUp,
  Coins,
  Download,
  Upload,
} from "lucide-react";
import {
  formatBytes,
  formatCurrency,
  formatExpire,
  formatPrice,
  remainingAssetValue,
  formatSpeed,
  isOnline,
  number,
  percent,
  trafficUsed,
} from "../format";
import type { Config, Server } from "../types";
import { ui } from "../locale";
import { carrierSelection, themeToggle } from "../theme";
import { useNodeLatency, type CarrierLatencyRow, type LatencyBar } from "../hooks/useNodeLatency";
import { Flag } from "./Flag";
import { OSIcon } from "./OSIcon";
import { ProgressBar } from "./ProgressBar";

function Metric({ label, value, used, sub, muted = false, valueTone = "" }: { label: string; value: string; used: number; sub: string; muted?: boolean; valueTone?: string }) {
  return (
    <div className={`metric ${muted ? "muted" : ""}`}>
      <div><span>{label}</span><strong className={valueTone}>{value}</strong></div>
      <ProgressBar value={used} />
      <small title={sub}>{sub}</small>
    </div>
  );
}

function CompactLine({ icon, children, tone = "" }: { icon: React.ReactNode; children: React.ReactNode; tone?: string }) {
  return <b className={tone}><span>{icon}</span><em>{children}</em></b>;
}

function QualityBars({ bars }: { bars: LatencyBar[] }) {
  return (
    <div className="quality-bars" style={{ gridTemplateColumns: `repeat(${Math.max(1, bars.length)}, minmax(0, 1fr))` }}>
      {bars.map((bar, index) => {
        const alignment = bars.length === 1 || (index >= 3 && index < bars.length - 3)
          ? "center"
          : index < 3 ? "start" : "end";
        return <span className="quality-bar" key={bar.key}>
          <i className={bar.tone} />
          <small className={`quality-tooltip ${alignment}`} role="tooltip">{bar.tooltip}</small>
        </span>;
      })}
    </div>
  );
}

function QualityPanel({ label, value, bars }: {
  label: string;
  value: string;
  bars: LatencyBar[];
}) {
  // 不挂 aria-label：label 和 value 已经是下面的可见文本，重复；
  // 而且整张卡片是 <button>，role=button 的子树在无障碍树里会被整体裁掉，挂了也读不到。
  return (
    <div className="quality-panel">
      <div><span>{label}</span><b>{value}</b></div>
      <QualityBars bars={bars} />
    </div>
  );
}

/** One column per metric, one row per carrier line. */
function CarrierPanel({ label, rows, kind, empty }: {
  label: string;
  rows: CarrierLatencyRow[];
  kind: "latency" | "loss";
  empty: string;
}) {
  return (
    <div className="quality-panel carrier-panel">
      <div><span>{label}</span></div>
      {rows.length ? <div className="carrier-rows">
        {rows.map((row) => <div className="carrier-row" key={row.id}>
          <div className="carrier-row-head" title={row.name}>
            <i style={{ backgroundColor: row.color }} />
            <span>{row.label}</span>
            <b>{kind === "latency" ? row.latencyDisplay : row.lossDisplay}</b>
          </div>
          <QualityBars bars={kind === "latency" ? row.latencyBars : row.lossBars} />
        </div>)}
      </div> : <div className="empty-inline">{empty}</div>}
    </div>
  );
}

export function NodeCard({ server, config, onOpen }: { server: Server; config: Config; onOpen: () => void }) {
  const threshold = config.offline_threshold_seconds;
  const online = isOnline(server, threshold);
  const memory = percent(server.mem_used, server.mem_total);
  const disk = percent(server.disk_used, server.disk_total);
  const usedTraffic = trafficUsed(server);
  const traffic = server.traffic_limit > 0 ? Math.min(100, (usedTraffic / server.traffic_limit) * 100) : 0;
  const locale = config.locale;
  const showCarriers = themeToggle(config, "showCarrierLatency", false);
  // The hook keys its memo on the selection values, not the object identity,
  // so rebuilding this each render is free.
  const quality = useNodeLatency(server, config.show_latency, locale, showCarriers ? carrierSelection(config) : null);
  const price = formatPrice(server, locale);
  const remainingValue = remainingAssetValue(server.price, server.billing_cycle, server.expires_at);
  const showExpiryPanel = config.show_expiry || config.show_price;
  const carrierEmpty = quality.loading
    ? ui(locale, "加载中", "Loading")
    : ui(locale, "未匹配到线路", "No lines matched");
  const lastUpdated = new Date(number(server.timestamp) * 1000).toLocaleString(locale, { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false });

  // 名字后面带上在线状态：卡片是 <button>，子树被裁掉，这个 aria-label 是读屏唯一能拿到的字符串。
  // 状态在视觉上只由 status-dot 的颜色表达（WCAG 1.4.1 靠颜色传达信息），这里补成文字。
  // 只放名字 + 状态，不塞 CPU/内存：一屏几十张卡，念完就太长了，数值留给详情页。
  return (
    <button className={`node-card glass-panel ${online ? "" : "offline"}`} onClick={onOpen} type="button" aria-label={ui(locale, `${server.name}，${online ? "在线" : "离线"}，查看详情`, `${server.name}, ${online ? "online" : "offline"}, view details`)}>
      <header className="node-header">
        <span className={`status-dot ${online ? "online" : ""}`} />
        <strong title={server.name}>{server.name}</strong>
        <OSIcon os={server.os} size={16} />
        <Flag region={server.region} size={19} locale={locale} />
      </header>

      <div className="node-body">
        <div className="node-chips">
          {config.show_uptime ? <span>{ui(locale, `在线 ${Math.floor(number(server.uptime) / 86400)} 天`, `Online ${Math.floor(number(server.uptime) / 86400)} days`)}</span> : null}
          {config.show_price && price ? <span title={price}>{price}</span> : null}
        </div>

        <div className="metric-grid">
          <Metric label="CPU" value={`${number(server.cpu).toFixed(1)}%`} used={number(server.cpu)} sub={`${number(server.load1).toFixed(2)}, ${number(server.load5).toFixed(2)}, ${number(server.load15).toFixed(2)}`} muted={!online} />
          <Metric label={ui(locale, "内存", "Memory")} value={`${memory.toFixed(1)}%`} used={memory} sub={`${formatBytes(server.mem_used)} / ${formatBytes(server.mem_total)}`} muted={!online} />
          <Metric label={ui(locale, "硬盘", "Disk")} value={`${disk.toFixed(1)}%`} used={disk} sub={`${formatBytes(server.disk_used)} / ${formatBytes(server.disk_total)}`} muted={!online} />
          <Metric label={ui(locale, "流量", "Traffic")} value={server.traffic_limit > 0 ? `${traffic.toFixed(1)}%` : "∞"} used={traffic} sub={`${formatBytes(usedTraffic)} / ${server.traffic_limit > 0 ? formatBytes(server.traffic_limit) : "∞"}`} muted={!online} valueTone={server.traffic_limit <= 0 ? "" : traffic >= 95 ? "danger-text" : traffic >= 60 ? "warning-text" : "success-text"} />
        </div>

        {/* 这三个面板不挂 aria-label：无 role 的 div 是 generic，规范禁止命名；就算补上
            role="group"，整张卡片是 <button>，role=button 的子树在无障碍树里会被整体裁掉，
            里面加什么都读不到，所以移除而不是修补。读屏用户拿不到这些数值，真正的出路是
            把卡片从 <button> 改成内容 + 覆盖层按钮，会动 CSS 和焦点行为，暂未做。 */}
        <div className={`data-grid ${showExpiryPanel ? "" : "two-columns"}`}>
          <div className="data-panel" title={ui(locale, "实时速率", "Live speed")}>
            <CompactLine icon={<ChevronUp size={11} />} tone="success-text">{formatSpeed(server.net_out)}</CompactLine>
            <CompactLine icon={<ChevronDown size={11} />} tone="info-text">{formatSpeed(server.net_in)}</CompactLine>
          </div>
          <div className="data-panel" title={ui(locale, "累计流量", "Total traffic")}>
            <CompactLine icon={<Upload size={11} />}>{formatBytes(server.net_tx_total)}</CompactLine>
            <CompactLine icon={<Download size={11} />}>{formatBytes(server.net_rx_total)}</CompactLine>
          </div>
          {showExpiryPanel ? <div className="data-panel" title={ui(locale, "剩余周期", "Billing cycle")}>
            {config.show_expiry ? <CompactLine icon={<CalendarDays size={11} />}>{formatExpire(server, locale)}</CompactLine> : null}
            {config.show_price ? <CompactLine icon={<Coins size={11} />}>{server.price === -1
              ? ui(locale, "免费", "Free")
              : server.price > 0 ? formatCurrency(remainingValue, server.currency) : ui(locale, "未设置", "Not set")}</CompactLine> : null}
          </div> : null}
        </div>

        {config.show_latency ? <div className="quality-grid">
          {showCarriers ? <>
            <CarrierPanel label={ui(locale, "延迟", "Latency")} rows={quality.carriers} kind="latency" empty={carrierEmpty} />
            <CarrierPanel label={ui(locale, "丢包", "Packet loss")} rows={quality.carriers} kind="loss" empty={carrierEmpty} />
          </> : <>
            <QualityPanel label={ui(locale, "延迟", "Latency")} value={quality.latencyDisplay} bars={quality.latencyBars} />
            <QualityPanel label={ui(locale, "丢包", "Packet loss")} value={quality.lossDisplay} bars={quality.lossBars} />
          </>}
        </div> : null}

        {!online ? <div className="node-offline-overlay" aria-hidden="true">
          <span>{ui(locale, "离线", "Offline")}</span>
          <small>{server.timestamp
            ? ui(locale, `最后更新 ${lastUpdated}`, `Last update ${lastUpdated}`)
            : ui(locale, "暂无更新时间", "No update yet")}</small>
        </div> : null}
      </div>
    </button>
  );
}
