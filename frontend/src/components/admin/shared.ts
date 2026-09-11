import { AlertTriangle, Database, Eye, Info, Palette, RadioTower, ServerCog, ShieldCheck, SlidersHorizontal, Terminal } from "lucide-react";
import { type AdminTab } from "../adminRoutes";
import { ui, type UiLocale } from "../locale";
import { type AdminServer, type DatabaseStats, type RemoteTask, type ServerInput, type Settings } from "../types";
import pkg from "../../package.json";

export const VERSION = import.meta.env.VITE_NODEFLARE_VERSION || pkg.version;
export const AGENT_SCRIPT_BASE = "https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent";

export type AgentPlatform = "linux" | "windows" | "macos" | "freebsd";
export type ThemeSourceMode = "repository" | "upload";

export interface AgentInstallInfo {
  agent_token: string;
  agent_mirror: string;
}

export function adminPages(locale: UiLocale | string | undefined): Record<AdminTab, { title: string; description: string }> {
  return {
    servers: { title: ui(locale, "服务器", "Servers"), description: ui(locale, "管理监控节点和运行参数", "Manage monitored nodes and runtime parameters") },
    latency: { title: ui(locale, "延迟检测", "Latency checks"), description: ui(locale, "配置分配给各服务器的 TCP 与 ICMP 延迟任务", "Configure TCP and ICMP latency tasks assigned to each server") },
    appearance: { title: ui(locale, "站点设置", "Site settings"), description: ui(locale, "调整站点信息、公开内容和前台显示项目", "Adjust site info, public content and dashboard display options") },
    themes: { title: ui(locale, "主题商店", "Theme store"), description: ui(locale, "选择内置主题或安装本地主题包", "Choose the built-in theme or install a local theme package") },
    themeSettings: { title: ui(locale, "主题设置", "Theme settings"), description: ui(locale, "调整当前前端主题提供的显示选项", "Adjust display options provided by the active frontend theme") },
    alerts: { title: ui(locale, "通知", "Notifications"), description: ui(locale, "配置 Telegram 通知和资源告警阈值", "Configure Telegram notifications and resource alert thresholds") },
    security: { title: ui(locale, "登录与安全", "Sign-in & security"), description: ui(locale, "管理管理员账号、登录设备和安全验证", "Manage the admin account, signed-in devices and security checks") },
    data: { title: ui(locale, "数据库", "Database"), description: ui(locale, "查看空间、备份恢复和迁移数据库", "Inspect usage, back up, restore and migrate the database") },
    remote: { title: ui(locale, "远程执行", "Remote execution"), description: ui(locale, "输入命令并执行", "Enter a command and run it") },
    about: { title: ui(locale, "关于", "About"), description: ui(locale, "版本信息与项目地址", "Version info and project links") },
  };
}

export function adminNavigation(locale: UiLocale | string | undefined) {
  return [
    { tab: "servers" as const, label: ui(locale, "服务器", "Servers"), icon: ServerCog },
    { tab: "latency" as const, label: ui(locale, "延迟检测", "Latency"), icon: RadioTower },
    { tab: "remote" as const, label: ui(locale, "远程执行", "Remote"), icon: Terminal },
    { tab: "alerts" as const, label: ui(locale, "通知", "Alerts"), icon: AlertTriangle },
    { tab: "themes" as const, label: ui(locale, "主题商店", "Themes"), icon: Palette },
    { tab: "themeSettings" as const, label: ui(locale, "主题设置", "Theme"), icon: SlidersHorizontal },
    { tab: "appearance" as const, label: ui(locale, "站点设置", "Site"), icon: Eye },
    { tab: "security" as const, label: ui(locale, "登录与安全", "Security"), icon: ShieldCheck },
    { tab: "data" as const, label: ui(locale, "数据库", "Database"), icon: Database },
    { tab: "about" as const, label: ui(locale, "关于", "About"), icon: Info },
  ];
}

export function remoteTaskStatusLabels(locale: UiLocale | string | undefined): Record<RemoteTask["status"], string> {
  return {
    pending: ui(locale, "等待接收", "Queued"),
    sent: ui(locale, "执行中", "Running"),
    success: ui(locale, "执行成功", "Succeeded"),
    failed: ui(locale, "执行失败", "Failed"),
  };
}


export const emptyServer: ServerInput = {
  name: "",
  region: "",
  group_name: "默认",
  tags: "",
  hidden: false,
  expires_at: null,
  traffic_limit: 0,
  traffic_limit_type: "sum",
  price: 0,
  billing_cycle: 30,
  currency: "CNY",
  auto_renewal: false,
  network_interface: "",
  reset_day: 1,
  report_interval: 60,
  collect_interval: 3,
  rx_correction: 0,
  tx_correction: 0,
  agent_mirror: "",
  offline_notify_disabled: false,
  auto_update: true,
};

export function toInput(server: AdminServer): ServerInput {
  return {
    ...emptyServer,
    name: server.name,
    region: server.region,
    group_name: server.group_name,
    tags: server.tags,
    hidden: server.hidden,
    expires_at: server.expires_at,
    traffic_limit: server.traffic_limit,
    traffic_limit_type: server.traffic_limit_type,
    price: server.price,
    billing_cycle: server.billing_cycle,
    currency: server.currency,
    auto_renewal: server.auto_renewal,
    network_interface: server.network_interface,
    reset_day: server.reset_day,
    report_interval: server.report_interval,
    collect_interval: server.collect_interval,
    rx_correction: server.rx_correction,
    tx_correction: server.tx_correction,
    agent_mirror: server.agent_mirror,
    offline_notify_disabled: server.offline_notify_disabled,
    auto_update: server.auto_update,
  };
}

export function billingCycles(locale: UiLocale | string | undefined): Array<{ days: number; label: string }> {
  return [
    { days: 30, label: ui(locale, "月", "Monthly") },
    { days: 90, label: ui(locale, "季", "Quarterly") },
    { days: 180, label: ui(locale, "半年", "Semi-annually") },
    { days: 365, label: ui(locale, "年", "Yearly") },
    { days: 0, label: ui(locale, "一次性", "One-time") },
  ];
}


export function formatDate(value: number | null) {
  return value ? new Date(value * 1000).toISOString().slice(0, 10) : "";
}

export function formatSessionTime(value: number) {
  return new Date(value * 1000).toLocaleString();
}

export function describeLoginDevice(userAgent: string, locale: UiLocale | string | undefined) {
  const browser = userAgent.includes("Edg/") ? "Edge"
    : userAgent.includes("Firefox/") ? "Firefox"
      : userAgent.includes("Chrome/") ? "Chrome"
        : userAgent.includes("Safari/") ? "Safari"
          : ui(locale, "其他客户端", "Other client");
  const system = userAgent.includes("Android") ? "Android"
    : /iPhone|iPad/.test(userAgent) ? "iOS"
      : userAgent.includes("Windows") ? "Windows"
        : userAgent.includes("Mac OS") ? "macOS"
          : userAgent.includes("Linux") ? "Linux"
            : "";
  return system ? `${browser} · ${system}` : browser;
}

export function settingPatch(settings: Settings, key: keyof Settings, value: unknown) {
  return { ...settings, [key]: value } as Settings;
}

export function shellLiteral(value: string) {
  return `'${value.replaceAll("'", `'\\''`)}'`;
}

export function powershellLiteral(value: string) {
  return `'${value.replaceAll("'", "''")}'`;
}

export async function copyText(value: string) {
  if (navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(value);
      return;
    } catch {}
  }
  const textarea = document.createElement("textarea");
  textarea.value = value;
  textarea.readOnly = true;
  textarea.style.position = "fixed";
  textarea.style.opacity = "0";
  document.body.append(textarea);
  textarea.select();
  const copied = document.execCommand("copy");
  textarea.remove();
  if (!copied) throw new Error("copy failed");
}

export async function waitForDatabaseSwitch(targetKind: DatabaseStats["kind"], locale: UiLocale | string | undefined) {
  const deadline = Date.now() + 120_000;
  while (Date.now() < deadline) {
    await new Promise((resolve) => window.setTimeout(resolve, 750));
    try {
      const response = await fetch(`/api/admin/database?restart=${Date.now()}`, {
        cache: "no-store",
        credentials: "same-origin",
        signal: AbortSignal.timeout(2_500),
      });
      if (response.status === 401) return;
      if (response.ok) {
        const database = await response.json() as DatabaseStats;
        if (database.kind === targetKind && !database.restart_required) return;
      }
    } catch {}
  }
  throw new Error(ui(locale, "NodeFlare 未自动恢复，请在主机上检查服务状态", "NodeFlare did not come back automatically; check the service status on the host"));
}
