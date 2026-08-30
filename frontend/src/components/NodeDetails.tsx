import {
  Activity,
  ArrowLeft,
  Box,
  Cpu,
  Network,
  Radio,
  RadioTower,
  ServerCog,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  Area,
  AreaChart,
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api } from "../api";
import { chartGapLimit, chartTimeLabel, chartTimeRange, insertTimelineGaps } from "../chart";
import { demoHistory, demoLatencyHistory, demoLatencyTasks } from "../demo";
import { averageOf } from "../latency";
import { displayGpuDevices, formatBytes, formatCpuName, formatSpeed, formatUptime, isOnline, number } from "../format";
import type { HistoryPoint, LatencySample, LatencyTestPoint, LiveLatencyResult, Server } from "../types";
import { Flag, regionDisplayName } from "./Flag";
import { OSIcon } from "./OSIcon";
import { ui } from "../locale";

type ChartType = "load" | "latency";

const LATENCY_COLORS = ["#2563eb", "#db2777", "#ea7b1b", "#0f766e", "#7c3aed", "#0891b2"];
const REALTIME_WINDOW_SECONDS = 60 * 60;
const MAX_REALTIME_POINTS = 720;
const MAX_LATENCY_HISTORY_ROWS = 4000;

function historyPointFromServer(server: Server): HistoryPoint | null {
  if (!Number.isFinite(server.timestamp) || number(server.timestamp) <= 0) return null;
  return {
    timestamp: number(server.timestamp),
    cpu: number(server.cpu),
    load1: number(server.load1),
    load5: number(server.load5),
    load15: number(server.load15),
    mem_used: number(server.mem_used),
    mem_total: number(server.mem_total),
    swap_used: number(server.swap_used),
    swap_total: number(server.swap_total),
    disk_used: number(server.disk_used),
    disk_total: number(server.disk_total),
    net_in: number(server.net_in),
    net_out: number(server.net_out),
    net_rx_total: number(server.net_rx_total),
    net_tx_total: number(server.net_tx_total),
    processes: number(server.processes),
    tcp_connections: number(server.tcp_connections),
    udp_connections: number(server.udp_connections),
    gpu_usage: number(server.gpu_usage),
    disk_read_bps: number(server.disk_read_bps),
    disk_write_bps: number(server.disk_write_bps),
    disk_read_iops: number(server.disk_read_iops),
    disk_write_iops: number(server.disk_write_iops),
    disk_await_ms: number(server.disk_await_ms),
    disk_utilization: number(server.disk_utilization),
  };
}

function appendRealtimePoint(points: HistoryPoint[], point: HistoryPoint | null): HistoryPoint[] {
  if (!point) return points;
  const cutoff = point.timestamp - REALTIME_WINDOW_SECONDS;
  const next = points.filter((current) => current.timestamp >= cutoff && current.timestamp !== point.timestamp);
  next.push(point);
  next.sort((left, right) => left.timestamp - right.timestamp);
  return next.slice(-MAX_REALTIME_POINTS);
}

function InfoItem({ icon, label, value }: { icon: React.ReactNode; label: string; value: React.ReactNode }) {
  return <div className="info-item"><span>{icon}</span><div><small>{label}</small><strong>{value}</strong></div></div>;
}

function InfoGroup({ icon, title, children }: { icon: React.ReactNode; title: string; children: React.ReactNode }) {
  return <section className="info-group glass-panel"><h2>{icon}{title}</h2><div className="info-group-grid">{children}</div></section>;
}

function ChartCard({ title, value, children }: { title: string; value: string; children: React.ReactNode }) {
  return <div className="detail-chart glass-panel"><header><h3>{title}</h3><span>{value}</span></header><div>{children}</div></div>;
}

function resourceTicks(total: number): number[] {
  return Array.from({ length: 5 }, (_, index) => total * index / 4);
}

function latencyBucketSeconds(hours: number, taskCount: number): number {
  const boundedHours = Math.max(1, Math.min(24 * 30, Math.trunc(hours)));
  const boundedTasks = Math.max(1, Math.min(128, Math.trunc(taskCount)));
  const base = boundedHours === 1 ? 60
    : boundedHours <= 4 ? 120
      : boundedHours <= 24 ? 600
        : boundedHours <= 168 ? 3600
          : boundedHours <= 720 ? 14_400
            : 86_400;
  const pointsPerTask = Math.max(1, Math.floor(MAX_LATENCY_HISTORY_ROWS / boundedTasks));
  return Math.max(base, Math.ceil(boundedHours * 3600 / pointsPerTask));
}

function mergeLatencySamples(current: LatencySample[], incoming: LatencySample[], hours: number, taskCount: number): LatencySample[] {
  const validIncoming = incoming.filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0);
  if (!validIncoming.length) return current.filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0);
  const latest = Math.max(...validIncoming.map((point) => point.timestamp));
  const cutoff = latest - hours * 3600;
  const bucket = latencyBucketSeconds(hours, taskCount);
  const key = (point: LatencySample) => `${point.task_id}:${Math.floor(point.timestamp / bucket)}`;
  const next = new Map(current
    .filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0 && point.timestamp >= cutoff)
    .map((point) => [key(point), point]));
  for (const point of validIncoming) {
    if (point.timestamp < cutoff) continue;
    const currentPoint = next.get(key(point));
    if (!currentPoint || point.timestamp >= currentPoint.timestamp) next.set(key(point), point);
  }
  return Array.from(next.values()).sort((left, right) => left.timestamp - right.timestamp);
}

interface LatencyChartPoint {
  timestamp: number;
  label: string;
  value: number | null;
}

interface LoadChartPoint {
  timestamp: number;
  cpu: number | null;
  mem_used: number | null;
  mem_total: number;
  disk_used: number | null;
  disk_total: number;
  net_in: number | null;
  net_out: number | null;
}

function insertLatencyGaps(points: LatencyChartPoint[], hours: number): LatencyChartPoint[] {
  return insertTimelineGaps(
    points,
    (timestamp) => ({ timestamp, label: "", value: null }),
    { minGap: 10_000, maxGap: chartGapLimit(hours) },
  );
}

function insertLoadGaps(points: HistoryPoint[], hours: number): LoadChartPoint[] {
  const mapped = points
    .filter((point) => Number.isFinite(point.timestamp) && point.timestamp > 0)
    .sort((left, right) => left.timestamp - right.timestamp)
    .map((point) => ({
      timestamp: point.timestamp * 1000,
      cpu: Number.isFinite(point.cpu) ? point.cpu : null,
      mem_used: Number.isFinite(point.mem_used) ? point.mem_used : null,
      mem_total: Math.max(0, number(point.mem_total)),
      disk_used: Number.isFinite(point.disk_used) ? point.disk_used : null,
      disk_total: Math.max(0, number(point.disk_total)),
      net_in: Number.isFinite(point.net_in) ? point.net_in : null,
      net_out: Number.isFinite(point.net_out) ? point.net_out : null,
    }));
  return insertTimelineGaps(
    mapped,
    (timestamp) => ({
      timestamp,
      cpu: null,
      mem_used: null,
      mem_total: 0,
      disk_used: null,
      disk_total: 0,
      net_in: null,
      net_out: null,
    }),
    { minGap: 10_000, maxGap: chartGapLimit(hours) },
  );
}

const tooltipStyle = {
  borderRadius: 8,
  border: "1px solid var(--border)",
  background: "var(--card-solid)",
  fontSize: 12,
};

export function NodeDetails({ server, liveLatencyResults, threshold, retentionDays, locale, demo = false, onClose }: {
  server: Server;
  liveLatencyResults: LiveLatencyResult[];
  threshold: number;
  retentionDays: number;
  locale: "zh-CN" | "en";
  demo?: boolean;
  onClose: () => void;
}) {
  const [loadHours, setLoadHours] = useState(0);
  const [latencyHours, setLatencyHours] = useState(1);
  const [chartType, setChartType] = useState<ChartType>("load");
  const [points, setPoints] = useState<HistoryPoint[]>([]);
  const [latencyPoints, setLatencyPoints] = useState<LatencySample[]>([]);
  const [latencyTasks, setLatencyTasks] = useState<LatencyTestPoint[]>([]);
  const [hiddenLatencyTaskIds, setHiddenLatencyTaskIds] = useState<Set<string>>(() => new Set());
  const [loadLoading, setLoadLoading] = useState(true);
  const [latencyLoading, setLatencyLoading] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [latencyError, setLatencyError] = useState("");
  const latestServerRef = useRef(server);
  latestServerRef.current = server;

  useEffect(() => {
    setLoadLoading(true);
    setLoadError("");
    const requestHours = loadHours === 0 ? 1 : loadHours;
    if (demo) {
      setPoints(demoHistory(server.id, requestHours));
      setLoadLoading(false);
      return;
    }
    let active = true;
    api.history(server.id, requestHours)
      .then((history) => {
        if (active) setPoints(loadHours === 0
          ? appendRealtimePoint(history.points, historyPointFromServer(latestServerRef.current))
          : history.points);
      })
      .catch((reason) => { if (active) { setPoints([]); setLoadError(reason instanceof Error ? reason.message : ui(locale, "历史数据加载失败", "Unable to load history")); } })
      .finally(() => { if (active) setLoadLoading(false); });
    return () => { active = false; };
  }, [server.id, loadHours, demo, locale]);

  useEffect(() => {
    setLatencyLoading(true);
    setLatencyError("");
    setLatencyTasks([]);
    setLatencyPoints([]);
    if (demo) {
      const taskIds = new Set(server.latency.map((point) => point.task_id));
      const tasks = demoLatencyTasks.filter((task) => taskIds.has(task.id));
      setLatencyTasks(tasks);
      setLatencyPoints(tasks.length ? demoLatencyHistory(server.id, latencyHours).filter((point) => taskIds.has(point.task_id)) : []);
      setLatencyLoading(false);
      return;
    }
    let active = true;
    api.latencyHistory(server.id, latencyHours)
      .then((latency) => { if (active) {
        setLatencyTasks(latency.tasks);
        setLatencyPoints((current) => mergeLatencySamples(
          current,
          latency.points,
          latencyHours,
          latency.tasks.length,
        ));
      } })
      .catch((reason) => { if (active) { setLatencyTasks([]); setLatencyPoints([]); setLatencyError(reason instanceof Error ? reason.message : ui(locale, "延迟数据加载失败", "Unable to load latency")); } })
      .finally(() => { if (active) setLatencyLoading(false); });
    return () => { active = false; };
  }, [server.id, latencyHours, demo, locale]);

  useEffect(() => {
    if (demo || !server.latency.length) return;
    setLatencyPoints((current) => mergeLatencySamples(
      current,
      server.latency,
      latencyHours,
      Math.max(latencyTasks.length, server.latency.length),
    ));
  }, [demo, latencyHours, latencyTasks.length, server.latency]);

  useEffect(() => {
    if (demo || !liveLatencyResults.length || !latencyTasks.length) return;
    const definitions = new Map(latencyTasks.map((task) => [task.id, task]));
    const samples = liveLatencyResults.flatMap((result): LatencySample[] => {
      const task = definitions.get(result.task_id);
      if (!task || !Number.isFinite(result.timestamp) || result.timestamp <= 0) return [];
      return [{
        task_id: task.id,
        server_id: server.id,
        name: task.name,
        task_type: task.task_type,
        target: task.target,
        port: task.port,
        timestamp: result.timestamp,
        latency_ms: result.latency_ms,
        packet_loss: result.packet_loss,
      }];
    });
    if (!samples.length) return;
    setLatencyPoints((current) => mergeLatencySamples(current, samples, latencyHours, latencyTasks.length));
  }, [demo, latencyHours, latencyTasks, liveLatencyResults, server.id]);

  useEffect(() => {
    setHiddenLatencyTaskIds((current) => {
      const available = new Set(latencyTasks.map((task) => task.id));
      const next = new Set([...current].filter((id) => available.has(id)));
      return next.size === current.size ? current : next;
    });
  }, [latencyTasks]);

  const online = isOnline(server, threshold);
  const gpuDevices = useMemo(() => displayGpuDevices(server.gpus), [server.gpus]);
  const gpuNames = gpuDevices.map((gpu) => gpu.model).join(" · ");
  const loadRanges = [
    { value: 0, label: ui(locale, "实时", "Live") },
    { value: 1, label: ui(locale, "1 小时", "1 hour") },
    { value: 4, label: ui(locale, "4 小时", "4 hours") },
    { value: 24, label: ui(locale, "1 天", "1 day") },
    { value: 168, label: ui(locale, "7 天", "7 days") },
    { value: 720, label: ui(locale, "30 天", "30 days") },
  ].filter((range) => range.value <= Math.max(24, retentionDays * 24));
  const ranges = chartType === "load" ? loadRanges : loadRanges.filter((range) => range.value > 0);
  const hours = chartType === "load" ? loadHours : latencyHours;
  const loadChartHours = loadHours === 0 ? 1 : loadHours;
  const data = useMemo(
    () => insertLoadGaps(points, loadChartHours),
    [loadChartHours, points],
  );
  // 有时间范围时钉住整段窗口，部分数据也保持真实比例；实时档跟随数据自适应
  // （与延迟图、Komari 原版一致）。points 进 deps 是为了在数据落位后重算
  // domain，慢查询时末端不会被裁掉。
  const loadTimeRange = useMemo(
    () => (loadHours === 0 ? null : chartTimeRange(loadChartHours)),
    [loadChartHours, loadHours, points],
  );
  const latencySeries = useMemo(() => {
    return latencyTasks.map((task, index) => {
      const samples = latencyPoints.filter((point) => point.task_id === task.id);
      const rawPoints: LatencyChartPoint[] = samples
        .filter((sample) => Number.isFinite(sample.timestamp) && sample.timestamp > 0)
        .sort((left, right) => left.timestamp - right.timestamp)
        .map((sample) => ({
          timestamp: sample.timestamp * 1000,
          label: chartTimeLabel(sample.timestamp * 1000, hours, locale),
          value: sample.latency_ms >= 0 ? sample.latency_ms : null,
        }));
      return {
        id: task.id,
        name: task.name,
        color: LATENCY_COLORS[index % LATENCY_COLORS.length],
        latency: averageOf(samples.map((sample) => sample.latency_ms)),
        loss: averageOf(samples.map((sample) => sample.packet_loss)),
        points: insertLatencyGaps(rawPoints, hours),
      };
    });
  }, [hours, latencyPoints, latencyTasks, locale]);
  const visibleLatencySeries = useMemo(
    () => latencySeries.filter((series) => !hiddenLatencyTaskIds.has(series.id)),
    [hiddenLatencyTaskIds, latencySeries],
  );
  const latencyData = useMemo(() => {
    const timestamps = new Set<number>();
    for (const series of visibleLatencySeries) {
      for (const point of series.points) timestamps.add(point.timestamp);
    }
    return Array.from(timestamps)
      .sort((left, right) => left - right)
      .map((timestamp) => ({ timestamp, label: chartTimeLabel(timestamp, hours, locale) }));
  }, [hours, locale, visibleLatencySeries]);
  const latencyTimeRange = useMemo(() => chartTimeRange(latencyHours), [latencyHours, latencyPoints]);
  const last = data[data.length - 1];
  const pingEnabled = latencyTasks.length > 0;
  const toggleLatencyTask = (taskId: string) => {
    setHiddenLatencyTaskIds((current) => {
      const next = new Set(current);
      if (next.has(taskId)) next.delete(taskId);
      else next.add(taskId);
      return next;
    });
  };

  return (
    <div className="detail-page">
      <section className="detail-hero glass-panel">
        <button className="icon-btn" onClick={onClose} title={ui(locale, "返回首页", "Back")}><ArrowLeft size={19} /></button>
        <Flag region={server.region} size={24} className="detail-flag" locale={locale} />
        <div className="detail-title">
          <div><h1>{server.name}</h1><span className={`status-chip ${online ? "online" : ""}`}>{online ? ui(locale, "在线", "Online") : ui(locale, "离线", "Offline")}</span></div>
          <p><OSIcon os={server.os} size={14} />{regionDisplayName(server.region, locale)}{server.group_name ? ` · ${server.group_name}` : ""}</p>
        </div>
      </section>

      <div className="info-groups">
        <InfoGroup title={ui(locale, "硬件信息", "Hardware")} icon={<Cpu size={16} />}>
          <InfoItem icon={<Cpu size={14} />} label="CPU" value={formatCpuName(server.cpu_model, server.cpu_cores)} />
          <InfoItem icon={<Box size={14} />} label={ui(locale, "架构", "Architecture")} value={server.arch || "--"} />
          <InfoItem icon={<Cpu size={14} />} label="GPU" value={gpuNames || ui(locale, "未检测到", "Not detected")} />
          <InfoItem icon={<ServerCog size={14} />} label={ui(locale, "虚拟化", "Virtualization")} value={server.virtualization || "--"} />
        </InfoGroup>
        <InfoGroup title={ui(locale, "系统信息", "System")} icon={<ServerCog size={16} />}>
          <InfoItem icon={<ServerCog size={14} />} label={ui(locale, "操作系统", "Operating system")} value={server.os || "--"} />
          <InfoItem icon={<Radio size={14} />} label={ui(locale, "运行时间", "Uptime")} value={formatUptime(server.uptime, locale)} />
          <InfoItem icon={<Cpu size={14} />} label={ui(locale, "内核版本", "Kernel")} value={server.kernel || "--"} />
          <InfoItem icon={<Network size={14} />} label={ui(locale, "进程 / 连接", "Processes / connections")} value={`${number(server.processes)} / ${number(server.tcp_connections) + number(server.udp_connections)}`} />
        </InfoGroup>
      </div>

      <section className="chart-section">
        <div className="chart-controls">
          {/* role="group" 是让 aria-label 生效的前提：无 role 的 div 是 generic，
              规范禁止给它命名，浏览器会把 aria-label 丢掉。下面几处同理。
              选中态用 aria-pressed 而不是 role="radio"：radio 组要求方向键 + roving tabindex，
              这里保持 Tab 逐个走的现状。同文件延迟任务图例用的也是 aria-pressed。 */}
          <div className="segmented" role="group" aria-label={ui(locale, "图表类型", "Chart type")}>
            <button className={chartType === "load" ? "active" : ""} aria-pressed={chartType === "load"} onClick={() => setChartType("load")}><Activity size={14} />{ui(locale, "负载", "Load")}</button>
            {pingEnabled ? <button className={chartType === "latency" ? "active" : ""} aria-pressed={chartType === "latency"} onClick={() => setChartType("latency")}><RadioTower size={14} />{ui(locale, "延迟", "Latency")}</button> : null}
          </div>
          <div className="segmented range-control" role="group" aria-label={ui(locale, "时间范围", "Time range")}>
            {ranges.map((range) => <button className={hours === range.value ? "active" : ""} aria-pressed={hours === range.value} key={range.value} onClick={() => chartType === "load" ? setLoadHours(range.value) : setLatencyHours(range.value)}>{range.label}</button>)}
          </div>
        </div>

        {chartType === "load" && loadError ? <div className="error-band">{loadError}</div> : null}
        {chartType === "latency" && latencyError ? <div className="error-band">{latencyError}</div> : null}
        {chartType === "load" && loadLoading ? <div className="chart-loading">{ui(locale, "正在读取历史数据", "Loading history")}</div> : chartType === "latency" && latencyLoading ? <div className="chart-loading">{ui(locale, "正在读取延迟数据", "Loading latency")}</div> : chartType === "load" && !data.length ? <div className="chart-loading">{ui(locale, "暂无历史数据", "No history")}</div> : chartType === "load" ? (
          <div className="detail-charts-grid">
            <ChartCard title="CPU" value={`${number(last.cpu).toFixed(1)}%`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="timestamp" type="number" domain={loadTimeRange ? loadTimeRange.domain : undefined} ticks={loadTimeRange?.ticks} allowDataOverflow={loadHours > 0} tick={{ fontSize: 10 }} minTickGap={30} tickFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} /><YAxis domain={[0, 100]} tick={{ fontSize: 10 }} width={36} /><Tooltip contentStyle={tooltipStyle} labelFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} formatter={(value) => [`${Number(value).toFixed(1)}%`, "CPU"]} /><Area type="monotone" dataKey="cpu" stroke="var(--danger-bar)" fill="var(--danger-bar)" fillOpacity={0.15} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
            <ChartCard title={ui(locale, "内存", "Memory")} value={`${formatBytes(last.mem_used)} / ${formatBytes(last.mem_total)}`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="timestamp" type="number" domain={loadTimeRange ? loadTimeRange.domain : undefined} ticks={loadTimeRange?.ticks} allowDataOverflow={loadHours > 0} tick={{ fontSize: 10 }} minTickGap={30} tickFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} /><YAxis domain={[0, last.mem_total]} ticks={resourceTicks(last.mem_total)} tick={{ fontSize: 10 }} width={56} tickFormatter={(value) => formatBytes(Number(value), 0)} /><Tooltip contentStyle={tooltipStyle} labelFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} formatter={(value) => [formatBytes(Number(value)), "内存"]} /><Area type="monotone" dataKey="mem_used" stroke="var(--primary)" fill="var(--primary)" fillOpacity={0.15} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
            <ChartCard title={ui(locale, "硬盘", "Disk")} value={`${formatBytes(last.disk_used)} / ${formatBytes(last.disk_total)}`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="timestamp" type="number" domain={loadTimeRange ? loadTimeRange.domain : undefined} ticks={loadTimeRange?.ticks} allowDataOverflow={loadHours > 0} tick={{ fontSize: 10 }} minTickGap={30} tickFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} /><YAxis domain={[0, last.disk_total]} ticks={resourceTicks(last.disk_total)} tick={{ fontSize: 10 }} width={56} tickFormatter={(value) => formatBytes(Number(value), 0)} /><Tooltip contentStyle={tooltipStyle} labelFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} formatter={(value) => [formatBytes(Number(value)), "硬盘"]} /><Area type="monotone" dataKey="disk_used" stroke="var(--warning-bar)" fill="var(--warning-bar)" fillOpacity={0.15} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
            <ChartCard title={ui(locale, "网络", "Network")} value={`↑ ${formatSpeed(last.net_out)}  ↓ ${formatSpeed(last.net_in)}`}>
              <ResponsiveContainer width="100%" height="100%"><AreaChart data={data}><CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" /><XAxis dataKey="timestamp" type="number" domain={loadTimeRange ? loadTimeRange.domain : undefined} ticks={loadTimeRange?.ticks} allowDataOverflow={loadHours > 0} tick={{ fontSize: 10 }} minTickGap={30} tickFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} /><YAxis tick={{ fontSize: 10 }} width={48} tickFormatter={(value) => formatBytes(Number(value), 0)} /><Tooltip contentStyle={tooltipStyle} labelFormatter={(value) => chartTimeLabel(Number(value), loadChartHours, locale)} formatter={(value, name) => [formatSpeed(Number(value)), name === "net_out" ? "上行" : "下行"]} /><Area type="monotone" dataKey="net_out" stroke="var(--success-bar)" fill="var(--success-bar)" fillOpacity={0.12} strokeWidth={2} dot={false} isAnimationActive={false} /><Area type="monotone" dataKey="net_in" stroke="var(--info)" fill="var(--info)" fillOpacity={0.1} strokeWidth={2} dot={false} isAnimationActive={false} /></AreaChart></ResponsiveContainer>
            </ChartCard>
          </div>
        ) : (
          <section className="latency-overview glass-panel">
            <div className="latency-summary-grid" style={{ gridTemplateColumns: `repeat(${Math.max(1, latencySeries.length)}, minmax(180px, 1fr))` }}>
              {latencySeries.map((series) => <div className={`latency-summary ${hiddenLatencyTaskIds.has(series.id) ? "hidden" : ""}`} key={series.id}>
                <span><i style={{ backgroundColor: series.color }} />{series.name}</span>
                <strong>{series.latency == null ? "--" : `${series.latency.toFixed(2)} ms`}</strong>
                <small>{series.loss == null ? "--" : `${series.loss.toFixed(2)}%`} 平均丢包</small>
              </div>)}
            </div>
            <div className="latency-task-legend" role="group" aria-label={ui(locale, "延迟任务", "Latency tasks")}>
              {latencySeries.map((series) => {
                const hidden = hiddenLatencyTaskIds.has(series.id);
                return <button
                  className={`latency-task-toggle ${hidden ? "hidden" : ""}`}
                  key={series.id}
                  type="button"
                  aria-pressed={!hidden}
                  title={hidden ? ui(locale, "显示此任务", "Show this task") : ui(locale, "隐藏此任务", "Hide this task")}
                  onClick={() => toggleLatencyTask(series.id)}
                >
                  <i style={{ backgroundColor: series.color }} />
                  <span>{series.name}</span>
                </button>;
              })}
            </div>
            <div className="latency-line-chart">
              {latencyData.length && visibleLatencySeries.length ? <ResponsiveContainer width="100%" height="100%">
                <LineChart data={latencyData} margin={{ top: 10, right: 12, left: 4, bottom: 2 }}>
                  <CartesianGrid stroke="var(--chart-grid)" strokeDasharray="3 3" />
                  <XAxis dataKey="timestamp" type="number" domain={latencyTimeRange.domain} ticks={latencyTimeRange.ticks} allowDataOverflow tick={{ fontSize: 10 }} minTickGap={40} tickFormatter={(value) => chartTimeLabel(Number(value), latencyHours, locale)} />
                  <YAxis tick={{ fontSize: 10 }} width={48} unit="ms" />
                  <Tooltip contentStyle={tooltipStyle} labelFormatter={(value) => chartTimeLabel(Number(value), latencyHours, locale)} formatter={(value, name) => [value == null ? "--" : `${Number(value).toFixed(1)} ms`, name]} />
                  {visibleLatencySeries.map((series) => <Line key={series.id} data={series.points} type="monotone" dataKey="value" name={series.name} stroke={series.color} strokeWidth={2} dot={false} connectNulls={false} isAnimationActive={false} />)}
                </LineChart>
              </ResponsiveContainer> : <div className="latency-empty">{visibleLatencySeries.length ? ui(locale, "暂无延迟数据", "No latency data") : ui(locale, "已隐藏全部任务", "All tasks are hidden")}</div>}
            </div>
          </section>
        )}
      </section>

    </div>
  );
}
