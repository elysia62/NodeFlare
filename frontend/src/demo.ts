import type { Config, ExchangeRates, HistoryPoint, LatencySample, LatencyTestPoint, Server } from "./types";

export const demoConfig: Config = {
  site_name: "NodeFlare",
  site_description: "边缘节点与核心服务运行状态",
  site_announcement: "",
  logo_url: "",
  locale: "zh-CN",
  public_dashboard: true,
  offline_threshold_seconds: 180,
  history_retention_days: 30,
  default_theme: "system",
  active_theme_id: "builtin-nodeflare-glass",
  background_url: "",
  theme_options: {},
  show_search: true,
  show_groups: true,
  show_stats: true,
  show_assets: true,
  show_traffic: true,
  show_speed: true,
  show_price: true,
  show_expiry: true,
  show_latency: true,
  show_uptime: true,
  turnstile_enabled: false,
  turnstile_login_enabled: true,
  turnstile_site_key: "",
  password_client_salt: "nodeflare-demo-password-kdf",
};

export const demoExchangeRates: ExchangeRates = {
  base: "CNY",
  rates: {
    CNY: 1,
    USD: 0.139,
    CAD: 0.2086,
    EUR: 0.119,
    GBP: 0.103,
    JPY: 21.1,
    HKD: 1.09,
    RUB: 11.560694,
    CHF: 0.120661,
    INR: 14.248668,
    VND: 3875.968992,
    THB: 4.97107,
  },
  source: "demo",
  date: new Date().toISOString().slice(0, 10),
  fetched_at: Math.floor(Date.now() / 1000),
  stale: false,
};

const now = Math.floor(Date.now() / 1000);

export const demoLatencyTasks: LatencyTestPoint[] = [
  { id: "demo-hk", name: "香港 TCP", task_type: "tcp", target: "hk.example.com", port: 443, interval_seconds: 60 },
  { id: "demo-tokyo", name: "东京 ICMP", task_type: "icmp", target: "tokyo.example.com", port: null, interval_seconds: 60 },
  { id: "demo-sg", name: "新加坡 TCP", task_type: "tcp", target: "sg.example.com", port: 443, interval_seconds: 60 },
];

function demoLatestLatency(serverId: string): LatencySample[] {
  return demoLatencyTasks.map((task, index) => ({
    task_id: task.id,
    server_id: serverId,
    name: task.name,
    task_type: task.task_type,
    target: task.target,
    port: task.port,
    timestamp: now - 12,
    latency_ms: 38 + index * 7,
    packet_loss: index === 1 ? 0.4 : 0,
  }));
}

const baseServer: Server = {
  id: "",
  name: "",
  region: "",
  group_name: "默认",
  tags: "",
  expires_at: null,
  traffic_limit: 0,
  traffic_limit_type: "sum",
  price: 0,
  billing_cycle: 30,
  currency: "CNY",
  auto_renewal: false,
  reset_day: 1,
  timestamp: now - 12,
  cpu: 24,
  load1: 0.42,
  load5: 0.35,
  load15: 0.27,
  mem_used: 3.36 * 1024 ** 3,
  mem_total: 8 * 1024 ** 3,
  swap_used: 0,
  swap_total: 2 * 1024 ** 3,
  disk_used: 49.6 * 1024 ** 3,
  disk_total: 160 * 1024 ** 3,
  net_in: 5.8 * 1024 ** 2,
  net_out: 1.2 * 1024 ** 2,
  net_rx_total: 640 * 1024 ** 3,
  net_tx_total: 220 * 1024 ** 3,
  uptime: 182 * 86400,
  processes: 126,
  tcp_connections: 342,
  udp_connections: 18,
  cpu_cores: 4,
  cpu_model: "AMD EPYC 7B13",
  os: "Debian GNU/Linux 12",
  kernel: "6.1.0",
  arch: "x86_64",
  virtualization: "KVM",
  gpu_usage: 0,
  gpu_model: "",
  agent_version: "0.0.1",
  disk_read_bps: 6.4 * 1024 ** 2,
  disk_write_bps: 2.1 * 1024 ** 2,
  disk_read_iops: 128,
  disk_write_iops: 46,
  disk_await_ms: 1.2,
  disk_utilization: 8.4,
  disks: [],
  gpus: [],
  latency: [],
};

function node(input: Partial<Server> & Pick<Server, "id" | "name">): Server {
  const server = { ...baseServer, ...input };
  server.latency = input.latency ?? demoLatestLatency(server.id);
  return server;
}

export const demoServers: Server[] = [
  node({ id: "hongkong-edge", name: "香港 Edge 01", region: "HK", group_name: "边缘网络", tags: "主力,线路:BGP,用途:网站", price: 128, expires_at: now + 46 * 86400, traffic_limit: 2 * 1024 ** 4 }),
  node({ id: "tokyo-core", name: "东京 Core", region: "JP", group_name: "核心服务", tags: "主力,线路:Premium", price: 9.9, currency: "USD", expires_at: now + 46 * 86400, mem_total: 16 * 1024 ** 3, mem_used: 6.72 * 1024 ** 3, disk_total: 224 * 1024 ** 3, disk_used: 69.44 * 1024 ** 3, traffic_limit: 2 * 1024 ** 4, net_rx_total: 810 * 1024 ** 3, net_tx_total: 310 * 1024 ** 3, cpu: 31 }),
  node({ id: "singapore-data", name: "新加坡 Data", region: "SG", group_name: "数据服务", tags: "数据库,NVMe", price: 88, expires_at: now + 46 * 86400, mem_total: 24 * 1024 ** 3, mem_used: 10.08 * 1024 ** 3, disk_total: 288 * 1024 ** 3, disk_used: 89.28 * 1024 ** 3, traffic_limit: 2 * 1024 ** 4, net_rx_total: 980 * 1024 ** 3, net_tx_total: 400 * 1024 ** 3, cpu: 38 }),
  node({ id: "los-angeles-west", name: "洛杉矶 West", region: "US", group_name: "边缘网络", tags: "备用,线路:CN2", price: 24, currency: "USD", expires_at: now + 46 * 86400, disk_total: 352 * 1024 ** 3, disk_used: 109.12 * 1024 ** 3, traffic_limit: 2 * 1024 ** 4, net_rx_total: 1.12 * 1024 ** 4, net_tx_total: 490 * 1024 ** 3, cpu: 45 }),
  node({ id: "frankfurt-lab", name: "法兰克福 Lab", region: "DE", group_name: "实验服务", tags: "Lab,IPv6", price: -1, expires_at: null, mem_total: 16 * 1024 ** 3, mem_used: 6.72 * 1024 ** 3, cpu: 52, uptime: 134 * 86400 }),
  node({ id: "taipei-homelab", name: "台北 HomeLab", region: "TW", group_name: "家庭网络", tags: "HomeLab,自建", price: -1, expires_at: null, mem_total: 24 * 1024 ** 3, mem_used: 10.08 * 1024 ** 3, cpu: 59, uptime: 122 * 86400 }),
  node({ id: "london-archive", name: "伦敦 Archive", region: "GB", group_name: "存储服务", tags: "Archive,HDD", price: 18, currency: "EUR", billing_cycle: 365, expires_at: now + 110 * 86400, disk_total: 4 * 1024 ** 4, disk_used: 2.7 * 1024 ** 4, cpu: 66, latency: [] }),
  node({ id: "toronto-standby", name: "多伦多 Standby", region: "CA", group_name: "备用节点", tags: "Standby", price: 16, currency: "CAD", expires_at: now + 18 * 86400, timestamp: now - 640, cpu: 0, net_in: 0, net_out: 0 }),
];

export function demoHistory(serverId: string, hours: number): HistoryPoint[] {
  const count = Math.min(180, Math.max(30, hours * 6));
  const step = Math.max(60, Math.floor((hours * 3600) / count));
  const seed = [...serverId].reduce((sum, char) => sum + char.charCodeAt(0), 0);
  return Array.from({ length: count }, (_, index) => {
    const wave = Math.sin((index + seed) / 7);
    const pulse = Math.cos((index + seed) / 13);
    return {
      timestamp: now - (count - index - 1) * step,
      cpu: Math.max(2, Math.min(96, 34 + wave * 18 + pulse * 7)),
      load1: 0.8 + wave * 0.45,
      load5: 0.72 + wave * 0.32,
      load15: 0.64 + pulse * 0.24,
      mem_used: (6.2 + wave * 0.45) * 1024 ** 3,
      mem_total: 16 * 1024 ** 3,
      swap_used: 0.2 * 1024 ** 3,
      swap_total: 2 * 1024 ** 3,
      disk_used: (84 + index / count) * 1024 ** 3,
      disk_total: 224 * 1024 ** 3,
      net_in: Math.max(0, (8 + wave * 5) * 1024 ** 2),
      net_out: Math.max(0, (3 + pulse * 2) * 1024 ** 2),
      net_rx_total: (800 + index) * 1024 ** 3,
      net_tx_total: (300 + index * 0.5) * 1024 ** 3,
      processes: 142 + Math.round(wave * 8),
      tcp_connections: 380 + Math.round(pulse * 55),
      udp_connections: 21,
      gpu_usage: 0,
      disk_read_bps: Math.max(0, (5 + wave * 3) * 1024 ** 2),
      disk_write_bps: Math.max(0, (2 + pulse) * 1024 ** 2),
      disk_read_iops: 110 + wave * 30,
      disk_write_iops: 45 + pulse * 12,
      disk_await_ms: 1.2 + Math.abs(wave),
      disk_utilization: 8 + Math.abs(pulse) * 12,
    };
  });
}

export function demoLatencyHistory(serverId: string, hours: number): LatencySample[] {
  const count = Math.min(180, Math.max(30, hours * 6));
  const step = Math.max(60, Math.floor((hours * 3600) / count));
  const seed = [...serverId].reduce((sum, char) => sum + char.charCodeAt(0), 0);
  return Array.from({ length: count }, (_, index) => demoLatencyTasks.map((task, taskIndex) => {
    const wave = Math.sin((index + seed + taskIndex * 5) / 7);
    return {
      task_id: task.id,
      server_id: serverId,
      name: task.name,
      task_type: task.task_type,
      target: task.target,
      port: task.port,
      timestamp: now - (count - index - 1) * step,
      latency_ms: 38 + taskIndex * 7 + wave * (4 + taskIndex),
      packet_loss: (index + taskIndex * 9) % 37 === 0 ? 2 + taskIndex : 0,
    };
  })).flat();
}
