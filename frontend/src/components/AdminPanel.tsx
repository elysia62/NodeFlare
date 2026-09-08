import {
  AlertTriangle,
  Info,
  ArrowLeft,
  ArrowRightLeft,
  ChevronDown,
  ChevronUp,
  CircleAlert,
  CircleCheck,
  Coins,
  Copy,
  Database,
  Download,
  Eye,
  GripVertical,
  KeyRound,
  LogOut,
  MonitorSmartphone,
  Moon,
  Palette,
  Pencil,
  Plus,
  Power,
  RadioTower,
  RotateCw,
  Save,
  Search,
  ServerCog,
  ShieldCheck,
  SlidersHorizontal,
  Sun,
  Terminal,
  Trash2,
  Upload,
  Check,
  ExternalLink,
} from "lucide-react";
import { ChangeEvent, DragEvent, FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ADMIN_UNAUTHORIZED_EVENT, api, ApiError } from "../api";
import { adminTabFromPath, adminTabPaths, canonicalAdminPath, type AdminTab } from "../adminRoutes";
import { formatBytes, formatByteSize, parseByteSize } from "../format";
import { derivePassword } from "../password";
import { hasActiveRemoteTasks, isRemoteTaskActive, REMOTE_TASK_POLL_INTERVAL_MS, REMOTE_TASK_POLL_TIMEOUT_MS } from "../refresh";
import { ASSET_CURRENCIES, type AdminServer, type Config, type DatabaseMigrationResult, type DatabaseStats, type ExchangeRates, type LoginSession, type RemoteTask, type ServerInput, type Settings, type Theme, type ThemeSettingField, type ThemeSettingsSchema, type ThemeSettingValue, type TotpSetup, type TotpStatus } from "../types";
import { Checkbox } from "./Checkbox";
import { TurnstileWidget } from "./TurnstileWidget";
import { useDialog } from "./useDialog";
import { SiteLogo } from "./SiteLogo";
import { LatencyManager } from "./LatencyManager";
import { AlertRuleManager } from "./AlertRuleManager";
import { TelegramSettings } from "./TelegramSettings";
import { PasswordInput } from "./PasswordInput";
import { useVerification } from "./useVerification";
import { Flag } from "./Flag";
import pkg from "../../package.json";

const VERSION = import.meta.env.VITE_NODEFLARE_VERSION || pkg.version;
const AGENT_SCRIPT_BASE = "https://raw.githubusercontent.com/elysia62/NodeFlare/main/agent";

type AgentPlatform = "linux" | "windows" | "macos" | "freebsd";
type ThemeSourceMode = "repository" | "upload";

interface AgentInstallInfo {
  agent_token: string;
  agent_mirror: string;
}

const adminPages: Record<AdminTab, { title: string; description: string }> = {
  servers: { title: "服务器", description: "管理监控节点和运行参数" },
  latency: { title: "延迟检测", description: "配置分配给各服务器的 TCP 与 ICMP 延迟任务" },
  appearance: { title: "站点设置", description: "调整站点信息、公开内容和前台显示项目" },
  themes: { title: "主题商店", description: "选择内置主题或安装本地主题包" },
  themeSettings: { title: "主题设置", description: "调整当前前端主题提供的显示选项" },
  alerts: { title: "通知", description: "配置 Telegram 通知和资源告警阈值" },
  security: { title: "登录与安全", description: "管理管理员账号、登录设备和安全验证" },
  data: { title: "数据库", description: "查看空间、备份恢复和迁移数据库" },
  remote: { title: "远程执行", description: "输入命令并执行" },
  about: { title: "关于", description: "版本信息与项目地址" },
};

const adminNavigation = [
  { tab: "servers" as const, label: "服务器", icon: ServerCog },
  { tab: "latency" as const, label: "延迟检测", icon: RadioTower },
  { tab: "remote" as const, label: "远程执行", icon: Terminal },
  { tab: "alerts" as const, label: "通知", icon: AlertTriangle },
  { tab: "themes" as const, label: "主题商店", icon: Palette },
  { tab: "themeSettings" as const, label: "主题设置", icon: SlidersHorizontal },
  { tab: "appearance" as const, label: "站点设置", icon: Eye },
  { tab: "security" as const, label: "登录与安全", icon: ShieldCheck },
  { tab: "data" as const, label: "数据库", icon: Database },
  { tab: "about" as const, label: "关于", icon: Info },
];

const remoteTaskStatusLabels: Record<RemoteTask["status"], string> = {
  pending: "等待接收",
  sent: "执行中",
  success: "执行成功",
  failed: "执行失败",
};


const emptyServer: ServerInput = {
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

function toInput(server: AdminServer): ServerInput {
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

const BILLING_CYCLES: Array<{ days: number; label: string }> = [
  { days: 30, label: "月" },
  { days: 90, label: "季" },
  { days: 180, label: "半年" },
  { days: 365, label: "年" },
  { days: 0, label: "一次性" },
];

function ServerIpMeta({ server, agentVersion, onCopy }: { server: AdminServer; agentVersion: string | null; onCopy: (ip: string) => void }) {
  const entries = [
    { family: "v4" as const, ip: server.ip_v4 || "" },
    { family: "v6" as const, ip: server.ip_v6 || "" },
  ].filter((entry) => entry.ip);
  return (
    <div className="server-name-meta">
      {entries.map((entry) => (
        <span className="server-ip-entry" key={entry.family}>
          <span className={`ip-badge ${entry.family}`}>{entry.family === "v6" ? "IPv6" : "IPv4"}</span>
          <button type="button" className="ip-value" title={`点击复制：${entry.ip}`} onClick={() => onCopy(entry.ip)}>{entry.ip}</button>
        </span>
      ))}
      {!entries.length ? <span className="meta-item">尚未探测到公网 IP</span> : null}
      <span className="meta-dot">·</span>
      <span className="meta-item">{agentVersion ? `Agent v${agentVersion}` : "Agent 未上报版本"}</span>
    </div>
  );
}

function formatDate(value: number | null) {
  return value ? new Date(value * 1000).toISOString().slice(0, 10) : "";
}

function formatSessionTime(value: number) {
  return new Date(value * 1000).toLocaleString();
}

function describeLoginDevice(userAgent: string) {
  const browser = userAgent.includes("Edg/") ? "Edge"
    : userAgent.includes("Firefox/") ? "Firefox"
      : userAgent.includes("Chrome/") ? "Chrome"
        : userAgent.includes("Safari/") ? "Safari"
          : "其他客户端";
  const system = userAgent.includes("Android") ? "Android"
    : /iPhone|iPad/.test(userAgent) ? "iOS"
      : userAgent.includes("Windows") ? "Windows"
        : userAgent.includes("Mac OS") ? "macOS"
          : userAgent.includes("Linux") ? "Linux"
            : "";
  return system ? `${browser} · ${system}` : browser;
}

function settingPatch(settings: Settings, key: keyof Settings, value: unknown) {
  return { ...settings, [key]: value } as Settings;
}

function shellLiteral(value: string) {
  return `'${value.replaceAll("'", `'\\''`)}'`;
}

function powershellLiteral(value: string) {
  return `'${value.replaceAll("'", "''")}'`;
}

async function copyText(value: string) {
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

async function waitForDatabaseSwitch(targetKind: DatabaseStats["kind"]) {
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
  throw new Error("NodeFlare 未自动恢复，请在主机上检查服务状态");
}

export function AdminPanel({
  config,
  dark,
  onToggleTheme,
  onChanged,
}: {
  config: Config;
  dark: boolean;
  onToggleTheme: () => void;
  onChanged: () => void;
}) {
  const [authenticated, setAuthenticated] = useState(false);
  const [authChecked, setAuthChecked] = useState(false);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [turnstileToken, setTurnstileToken] = useState("");
  const [turnstileReset, setTurnstileReset] = useState(0);
  const [loginTotpCode, setLoginTotpCode] = useState("");
  const [loginTotpChallenge, setLoginTotpChallenge] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [tab, setTab] = useState<AdminTab>(() => adminTabFromPath(window.location.pathname));
  const [servers, setServers] = useState<AdminServer[]>([]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [draggingId, setDraggingId] = useState("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [database, setDatabase] = useState<DatabaseStats | null>(null);
  const [databaseMigrationUrl, setDatabaseMigrationUrl] = useState("");
  const [databaseMigrationResult, setDatabaseMigrationResult] = useState<DatabaseMigrationResult | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [exchangeRates, setExchangeRates] = useState<ExchangeRates | null>(null);
  const [exchangeRatesExpanded, setExchangeRatesExpanded] = useState(false);
  const [themes, setThemes] = useState<Theme[]>([]);
  const [themeName, setThemeName] = useState("");
  const [themeDescription, setThemeDescription] = useState("");
  const [themeUrl, setThemeUrl] = useState("");
  const [themeSourceMode, setThemeSourceMode] = useState<ThemeSourceMode>("repository");
  const [themeFile, setThemeFile] = useState<File | null>(null);
  const [themeSettingsSchema, setThemeSettingsSchema] = useState<ThemeSettingsSchema | null>(null);
  const [editing, setEditing] = useState<AdminServer | "new" | null>(null);
  const [form, setForm] = useState<ServerInput>(emptyServer);
  const [install, setInstall] = useState<AgentInstallInfo | null>(null);
  const [priceText, setPriceText] = useState("0");
  const [trafficLimitText, setTrafficLimitText] = useState("0");
  const [rxCurrentText, setRxCurrentText] = useState("0");
  const [txCurrentText, setTxCurrentText] = useState("0");
  const [rxCurrentBytes, setRxCurrentBytes] = useState(0);
  const [txCurrentBytes, setTxCurrentBytes] = useState(0);
  const [rxBaseBytes, setRxBaseBytes] = useState(0);
  const [txBaseBytes, setTxBaseBytes] = useState(0);
  const [installPlatform, setInstallPlatform] = useState<AgentPlatform>("linux");
  const [remoteSelectedIds, setRemoteSelectedIds] = useState<string[]>([]);
  const [remoteQuery, setRemoteQuery] = useState("");
  const [remoteCommand, setRemoteCommand] = useState("");
  const [remoteTasks, setRemoteTasks] = useState<RemoteTask[]>([]);
  const [remotePollingUntil, setRemotePollingUntil] = useState(0);
  const [twoFactorStatus, setTwoFactorStatus] = useState<TotpStatus | null>(null);
  const [twoFactorSetup, setTwoFactorSetup] = useState<TotpSetup | null>(null);
  const [newPasswordConfirmation, setNewPasswordConfirmation] = useState("");
  const [twoFactorSecretCopied, setTwoFactorSecretCopied] = useState(false);
  const [loginSessions, setLoginSessions] = useState<LoginSession[]>([]);
  const [loginSessionsLoaded, setLoginSessionsLoaded] = useState(false);
  const [revokingSessionId, setRevokingSessionId] = useState("");
  const themeFileInputRef = useRef<HTMLInputElement>(null);
  const databaseRestoreInputRef = useRef<HTMLInputElement>(null);
  const remoteTasksRef = useRef<RemoteTask[]>([]);
  const remoteTasksRequestRef = useRef(0);
  const remoteTasksActive = useMemo(() => hasActiveRemoteTasks(remoteTasks), [remoteTasks]);
  const remotePayloadReady = Boolean(remoteCommand.trim());
  const loginTotpRequired = config.totp_login_enabled || loginTotpChallenge;
  const verificationDialog = useVerification(authenticated);

  remoteTasksRef.current = remoteTasks;

  const load = useCallback(async () => {
    setBusy(true);
    setError("");
    try {
      const [serverResult, settingsResult, themesResult, twoFactorResult] = await Promise.all([
        api.adminServers(),
        api.settings(),
        api.themes(),
        api.twoFactorStatus(),
      ]);
      setServers(serverResult.servers);
      const serverIds = new Set(serverResult.servers.map((server) => server.id));
      setRemoteSelectedIds((current) => current.filter((id) => serverIds.has(id)));
      setThemes(themesResult.themes);
      setSettings(settingsResult);
      setTwoFactorStatus(twoFactorResult);
      setAuthenticated(true);
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 401) {
        setAuthenticated(false);
        setError("");
        return;
      }
      setError(reason instanceof Error ? reason.message : "加载失败");
    } finally {
      setAuthChecked(true);
      setBusy(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  useEffect(() => {
    if (!authChecked) return;
    const syncAdminPath = () => {
      const canonicalPath = canonicalAdminPath(window.location.pathname, authenticated);
      if (window.location.pathname !== canonicalPath) {
        window.history.replaceState(null, "", canonicalPath);
      }
      setTab(adminTabFromPath(canonicalPath));
      setNotice("");
      setError("");
    };
    syncAdminPath();
    const handlePopState = () => syncAdminPath();
    window.addEventListener("popstate", handlePopState);
    return () => window.removeEventListener("popstate", handlePopState);
  }, [authChecked, authenticated]);

  useEffect(() => {
    if (!authenticated) return;
    if (tab === "themeSettings") void loadThemeSettings();
    if (tab === "data" && !database) void loadDatabase();
    if (tab === "security") void loadLoginSessions();
  }, [authenticated, tab]);

  useEffect(() => {
    const resetAuthentication = () => {
      setAuthenticated(false);
      setAuthChecked(true);
      setError("");
      setNotice("");
    };
    window.addEventListener(ADMIN_UNAUTHORIZED_EVENT, resetAuthentication);
    return () => window.removeEventListener(ADMIN_UNAUTHORIZED_EVENT, resetAuthentication);
  }, []);

  useEffect(() => {
    if (!notice) return;
    const timer = window.setTimeout(() => setNotice(""), 3200);
    return () => window.clearTimeout(timer);
  }, [notice]);

  useEffect(() => {
    if (!error) return;
    const timer = window.setTimeout(() => setError(""), 6000);
    return () => window.clearTimeout(timer);
  }, [error]);

  useEffect(() => {
    if (!twoFactorSecretCopied) return;
    const timer = window.setTimeout(() => setTwoFactorSecretCopied(false), 1800);
    return () => window.clearTimeout(timer);
  }, [twoFactorSecretCopied]);

  async function login(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const passwordDerived = await derivePassword(password, config.password_client_salt);
      await api.login(username.trim(), passwordDerived, turnstileToken, loginTotpCode);
      setPassword("");
      setTurnstileToken("");
      setLoginTotpCode("");
      setLoginTotpChallenge(false);
      setAuthenticated(true);
      await load();
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 428) setLoginTotpChallenge(true);
      setError(reason instanceof Error ? reason.message : "登录失败");
      setTurnstileToken("");
      setTurnstileReset((value) => value + 1);
    } finally { setBusy(false); }
  }

  function openEditor(server?: AdminServer) {
    setEditing(server ?? "new");
    setForm(server ? toInput(server) : { ...emptyServer });
    setPriceText(server ? String(server.price) : "0");
    setTrafficLimitText(formatByteSize(server?.traffic_limit ?? 0));
    const rxCurrent = server?.net_rx_total ?? 0;
    const txCurrent = server?.net_tx_total ?? 0;
    setRxCurrentText(formatByteSize(rxCurrent));
    setTxCurrentText(formatByteSize(txCurrent));
    setRxCurrentBytes(rxCurrent);
    setTxCurrentBytes(txCurrent);
    setRxBaseBytes(rxCurrent - (server?.rx_correction ?? 0));
    setTxBaseBytes(txCurrent - (server?.tx_correction ?? 0));
    setError("");
  }

  function updatePriceText(raw: string) {
    if (!/^\d*\.?\d*$/.test(raw)) return;
    setPriceText(raw);
    const parsed = Number(raw);
    if (Number.isFinite(parsed)) updateForm("price", parsed);
  }

  function sizeInputProps(text: string, setText: (value: string) => void, commit: (bytes: number) => void, current: number) {
    return {
      value: text,
      inputMode: "decimal" as const,
      onChange: (event: ChangeEvent<HTMLInputElement>) => {
        const raw = event.target.value;
        setText(raw);
        const parsed = parseByteSize(raw);
        if (parsed !== null) commit(parsed);
      },
      onBlur: () => setText(formatByteSize(current)),
    };
  }

  async function saveServer(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const payload: ServerInput = {
      ...form,
      rx_correction: Math.round(rxCurrentBytes - rxBaseBytes),
      tx_correction: Math.round(txCurrentBytes - txBaseBytes),
    };
    try {
      if (editing === "new") {
        const result = await api.createServer(payload);
        setInstall({ agent_token: result.agent_token, agent_mirror: form.agent_mirror });
      } else if (editing) {
        await api.updateServer(editing.id, payload);
      }
      setEditing(null);
      await load();
      onChanged();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "保存节点失败");
    } finally { setBusy(false); }
  }

  async function removeSelected() {
    if (!selectedIds.length || !window.confirm(`确认删除选中的 ${selectedIds.length} 个节点及其全部历史数据？`)) return;
    setBusy(true);
    try {
      await api.deleteServers(selectedIds);
      setSelectedIds([]);
      await load();
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "批量删除失败"); }
    finally { setBusy(false); }
  }

  async function remove(server: AdminServer) {
    if (!window.confirm(`确认删除“${server.name}”及其全部历史数据？`)) return;
    try { await api.deleteServer(server.id); await load(); onChanged(); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "删除失败"); }
  }

  async function showInstallCommand(server: AdminServer) {
    setBusy(true);
    setError("");
    try {
      const { agent_token } = await api.createAgentInstallToken(server.id);
      setInstall({ agent_token, agent_mirror: server.agent_mirror });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "生成 Agent 安装命令失败");
    } finally {
      setBusy(false);
    }
  }

  async function copyInstallCommand() {
    try {
      if (installCommand) await copyText(installCommand);
      setNotice("安装命令已复制");
    } catch {
      setError("复制失败，请手动选择命令复制");
    }
  }

  async function copyServerIp(ip: string) {
    try {
      await copyText(ip);
      setNotice("IP 已复制");
    } catch {
      setError("复制失败，请手动选择复制");
    }
  }

  async function move(index: number, offset: number) {
    const target = index + offset;
    if (target < 0 || target >= servers.length) return;
    const next = [...servers];
    [next[index], next[target]] = [next[target], next[index]];
    setServers(next);
    try { await api.reorderServers(next.map((server) => server.id)); onChanged(); }
    catch (reason) { setServers(servers); setError(reason instanceof Error ? reason.message : "排序失败"); }
  }

  function startDrag(event: DragEvent<HTMLButtonElement>, id: string) {
    setDraggingId(id);
    event.dataTransfer.effectAllowed = "move";
    event.dataTransfer.setData("text/plain", id);
  }

  async function dropServer(event: DragEvent<HTMLDivElement>, targetId: string) {
    event.preventDefault();
    const sourceId = draggingId || event.dataTransfer.getData("text/plain");
    setDraggingId("");
    if (!sourceId || sourceId === targetId) return;
    const sourceIndex = servers.findIndex((server) => server.id === sourceId);
    const targetIndex = servers.findIndex((server) => server.id === targetId);
    if (sourceIndex < 0 || targetIndex < 0) return;
    const previous = [...servers];
    const next = [...servers];
    const [moved] = next.splice(sourceIndex, 1);
    next.splice(targetIndex, 0, moved);
    setServers(next);
    try { await api.reorderServers(next.map((server) => server.id)); onChanged(); }
    catch (reason) { setServers(previous); setError(reason instanceof Error ? reason.message : "排序失败"); }
  }

  async function saveSite(event: FormEvent) {
    event.preventDefault();
    if (!settings) return;
    setBusy(true);
    setError("");
    setNotice("");
    const payload: Partial<Settings> = { ...settings };
    delete payload.admin_password_configured;
    delete payload.totp_login_enabled;
    delete payload.current_password_derived;
    delete payload.current_totp_code;
    try {
      if (payload.new_password) {
        if (payload.new_password !== newPasswordConfirmation) throw new Error("两次输入的新密码不一致");
        payload.new_password_derived = await derivePassword(payload.new_password, config.password_client_salt);
        delete payload.new_password;
      } else {
        delete payload.new_password;
        delete payload.new_password_derived;
      }
      if (tab === "security") {
        const proof = await sensitiveProof("保存设置");
        if (!proof) return;
        if (proof.totpCode) {
          payload.current_totp_code = proof.totpCode;
        } else {
          payload.current_password_derived = proof.passwordDerived;
        }
      }
      const result = await api.saveSettings(payload);
      setSettings(result);
      setNewPasswordConfirmation("");
      if (tab === "security") await loadLoginSessions();
      setNotice("设置已保存");
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "保存设置失败"); }
    finally { setBusy(false); }
  }

  async function loadDatabase() {
    setBusy(true); setError("");
    try {
      const [stats, rates] = await Promise.all([api.databaseStats(), api.exchangeRates()]);
      setDatabase(stats);
      if (!stats.restart_required) setDatabaseMigrationResult(null);
      setExchangeRates(rates);
    }
    catch (reason) { setError(reason instanceof Error ? reason.message : "读取数据库统计失败"); }
    finally { setBusy(false); }
  }

  async function sensitiveProof(action: string) {
    if (!twoFactorStatus) throw new Error("正在读取两步验证状态，请稍候");
    const entered = await verificationDialog.ask(action, twoFactorStatus.enabled);
    if (entered === null) return null;
    if (twoFactorStatus.enabled) return { totpCode: entered };
    return { passwordDerived: await derivePassword(entered, config.password_client_salt) };
  }

  async function reclaimDatabase() {
    if (!window.confirm("回收空间期间数据库会短暂不可用，确认继续？")) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const result = await api.reclaimDatabase();
      setDatabase(result.database);
      setNotice(result.reclaimed_bytes > 0 ? `已回收 ${formatBytes(result.reclaimed_bytes)}` : "数据库已整理，当前没有可释放空间");
    } catch (reason) { setError(reason instanceof Error ? reason.message : "回收数据库空间失败"); }
    finally { setBusy(false); }
  }

  async function migrateDatabase() {
    const databaseUrl = databaseMigrationUrl.trim();
    if (!databaseUrl) return;
    const source = database?.kind === "postgresql" ? "PostgreSQL" : "SQLite";
    const target = database?.kind === "postgresql" ? "SQLite" : "PostgreSQL";
    if (!window.confirm(`将 ${source} 迁移到 ${target}？目标库中已有的 NodeFlare 数据会被覆盖，完成后业务写入将暂停，直至重启服务。`)) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof("迁移数据库");
      if (!proof) return;
      const result = await api.migrateDatabase(databaseUrl, proof);
      setDatabaseMigrationUrl("");
      setDatabaseMigrationResult(result);
      setDatabase((current) => current ? { ...current, restart_required: result.restart_required } : current);
      const targetName = result.target_kind === "postgresql" ? "PostgreSQL" : "SQLite";
      setNotice(`数据库已迁移到 ${targetName}，共 ${result.migrated_rows.toLocaleString()} 行（${formatBytes(result.size_bytes)}），业务写入已暂停，请重启服务`);
    } catch (reason) { setError(reason instanceof Error ? reason.message : "数据库迁移失败"); }
    finally { setBusy(false); }
  }

  async function restartAfterDatabaseMigration() {
    if (!database?.restart_required || !window.confirm("立即重启 NodeFlare 并切换到新数据库？管理界面和 Agent 会短暂断开。")) return;
    const targetKind = databaseMigrationResult?.target_kind === "postgresql"
      ? "postgresql"
      : databaseMigrationResult?.target_kind === "sqlite"
        ? "sqlite"
        : database.kind === "postgresql" ? "sqlite" : "postgresql";
    setBusy(true); setRestarting(true); setError(""); setNotice("NodeFlare 正在重启");
    try {
      await api.restartAfterDatabaseMigration();
      await waitForDatabaseSwitch(targetKind);
      window.location.replace("/admin/login");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "重启 NodeFlare 失败");
    } finally {
      setBusy(false);
      setRestarting(false);
    }
  }

  async function refreshExchangeRates() {
    setBusy(true); setError(""); setNotice("");
    try {
      const rates = await api.refreshExchangeRates();
      setExchangeRates(rates);
      setNotice(`汇率已更新 · ${rates.source} · ${rates.date}`);
    } catch (reason) { setError(reason instanceof Error ? reason.message : "汇率更新失败"); }
    finally { setBusy(false); }
  }

  async function exportDatabaseBackup() {
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof("导出备份");
      if (!proof) return;
      const { blob, filename } = await api.databaseBackup(proof);
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = filename;
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      setNotice("数据库备份已导出");
    } catch (reason) { setError(reason instanceof Error ? reason.message : "导出数据库备份失败"); }
    finally { setBusy(false); }
  }

  async function restoreDatabaseBackup(file: File) {
    if (file.size > 512 * 1024 * 1024) {
      setError("数据库备份 ZIP 不能超过 512 MiB");
      return;
    }
    if (!window.confirm("恢复将覆盖当前数据库，并使现有登录会话失效。确认继续？")) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof("恢复备份");
      if (!proof) return;
      const result = await api.restoreDatabaseBackup(file, proof);
      window.alert(`数据库已恢复 ${result.restored_rows.toLocaleString()} 行，请使用备份中的管理员账号重新登录。`);
      window.location.reload();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "恢复数据库备份失败"); }
    finally { setBusy(false); }
  }

  async function loadThemeSettings() {
    setBusy(true); setError("");
    try { setThemeSettingsSchema(await api.themeSettings()); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "读取主题设置失败"); }
    finally { setBusy(false); }
  }

  async function addTheme(event: FormEvent) {
    event.preventDefault();
    if (themeSourceMode === "upload" && !themeFile) {
      setError("请选择 ZIP 主题文件");
      return;
    }
    if (themeSourceMode === "upload" && themeFile && themeFile.size > 32 * 1024 * 1024) {
      setError("主题 ZIP 不能超过 32 MiB");
      return;
    }
    setBusy(true); setError(""); setNotice("");
    try {
      if (themeSourceMode === "upload" && themeFile) {
        await api.uploadTheme({ name: themeName.trim(), description: themeDescription.trim() }, themeFile);
      } else {
        await api.addTheme({ name: themeName.trim(), description: themeDescription.trim(), url: themeUrl.trim() });
      }
      setThemeName(""); setThemeDescription(""); setThemeUrl("");
      setThemeFile(null);
      if (themeFileInputRef.current) themeFileInputRef.current.value = "";
      await load();
      setNotice(themeSourceMode === "upload" ? "主题 ZIP 已安装" : "最新 Release 主题已安装");
    } catch (reason) { setError(reason instanceof Error ? reason.message : "添加主题失败"); }
    finally { setBusy(false); }
  }

  async function activateTheme(theme: Theme) {
    if (theme.active) return;
    setBusy(true); setError(""); setNotice("");
    try {
      await api.activateTheme(theme.id);
      await load();
      setNotice(`已启用主题：${theme.name}`);
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "启用主题失败"); }
    finally { setBusy(false); }
  }

  async function previewTheme(theme: Theme) {
    if (theme.builtin) return;
    const previewWindow = window.open("", "_blank");
    setBusy(true); setError("");
    try {
      const { preview_url } = await api.previewTheme(theme.id);
      if (previewWindow) {
        previewWindow.opener = null;
        previewWindow.location.replace(preview_url);
      } else {
        window.open(preview_url, "_blank", "noopener,noreferrer");
      }
    } catch (reason) {
      previewWindow?.close();
      setError(reason instanceof Error ? reason.message : "创建主题预览失败");
    } finally { setBusy(false); }
  }

  async function removeTheme(theme: Theme) {
    if (theme.builtin || !window.confirm(`确认删除主题“${theme.name}”？`)) return;
    setBusy(true); setError(""); setNotice("");
    try {
      await api.deleteTheme(theme.id);
      await load();
      setNotice("主题已删除");
    } catch (reason) { setError(reason instanceof Error ? reason.message : "删除主题失败"); }
    finally { setBusy(false); }
  }

  async function loadLoginSessions() {
    try {
      const result = await api.loginSessions();
      setLoginSessions(result.sessions);
      setLoginSessionsLoaded(true);
    } catch (reason) {
      if (!(reason instanceof ApiError && reason.status === 401)) {
        setError(reason instanceof Error ? reason.message : "读取登录设备失败");
      }
    }
  }

  async function revokeLoginSession(session: LoginSession) {
    if (session.current || !window.confirm(`确认让“${describeLoginDevice(session.user_agent)}”下线？`)) return;
    setRevokingSessionId(session.id); setError(""); setNotice("");
    try {
      await api.revokeLoginSession(session.id);
      setLoginSessions((current) => current.filter((item) => item.id !== session.id));
      setNotice("设备已踢下线");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "设备下线失败");
    } finally { setRevokingSessionId(""); }
  }

  async function logout() {
    try { await api.logout(); }
    finally {
      setAuthenticated(false); setServers([]); setSettings(null); setSelectedIds([]);
      setTwoFactorStatus(null); setTwoFactorSetup(null); setNewPasswordConfirmation("");
      setTwoFactorSecretCopied(false);
      setLoginSessions([]); setLoginSessionsLoaded(false); setRevokingSessionId("");
      setRemoteSelectedIds([]); setRemoteCommand(""); setRemoteTasks([]); setRemoteQuery("");
      remoteTasksRequestRef.current += 1;
      remoteTasksRef.current = [];
      setRemotePollingUntil(0);
    }
  }

  async function setupTwoFactor() {
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof("生成两步验证密钥");
      if (!proof) return;
      const setup = await api.setupTwoFactor(proof);
      setTwoFactorSetup(setup);
      setTwoFactorStatus({ enabled: false, has_secret: true });
      setTwoFactorSecretCopied(false);
      setNotice("两步验证密钥已生成，请先加入验证器再启用");
    } catch (reason) { setError(reason instanceof Error ? reason.message : "生成两步验证密钥失败"); }
    finally { setBusy(false); }
  }

  async function enableTwoFactor() {
    setBusy(true); setError(""); setNotice("");
    try {
      const code = await verificationDialog.ask("启用两步验证", true);
      if (code === null) return;
      await api.enableTwoFactor(code);
      setTwoFactorStatus({ enabled: true, has_secret: true });
      setTwoFactorSetup(null);
      setTwoFactorSecretCopied(false);
      setNotice("两步验证已启用");
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "启用两步验证失败"); }
    finally { setBusy(false); }
  }

  async function disableTwoFactor() {
    setBusy(true); setError(""); setNotice("");
    try {
      const code = await verificationDialog.ask("禁用两步验证", true);
      if (code === null) return;
      await api.disableTwoFactor(code);
      setTwoFactorStatus({ enabled: false, has_secret: true });
      setNotice("两步验证已禁用");
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : "禁用两步验证失败"); }
    finally { setBusy(false); }
  }

  async function copyTwoFactorSecret() {
    if (!twoFactorSetup) return;
    try {
      await copyText(twoFactorSetup.secret);
      setTwoFactorSecretCopied(true);
    } catch {
      setError("复制失败，请手动选择复制");
    }
  }

  const installCommand = useMemo(() => {
    if (!install) return "";
    const origin = window.location.origin;
    const installer = AGENT_SCRIPT_BASE;
    const mirror = install.agent_mirror.trim().replace(/\/+$/, "");
    const shellMirror = mirror ? ` -m ${shellLiteral(mirror)}` : "";
    const powershellMirror = mirror ? ` -Mirror ${powershellLiteral(mirror)}` : "";
    if (installPlatform === "windows") {
      return `Invoke-WebRequest -UseBasicParsing -Uri "${installer}/install.ps1" -OutFile "$env:TEMP\\nodeflare-install.ps1"\nUnblock-File "$env:TEMP\\nodeflare-install.ps1"\n& "$env:TEMP\\nodeflare-install.ps1" -e ${powershellLiteral(origin)} -t ${powershellLiteral(install.agent_token)}${powershellMirror}`;
    }
    if (installPlatform === "macos") {
      return `curl -fsSL ${installer}/install-macos.sh | sudo sh -s -- -e ${shellLiteral(origin)} -t ${shellLiteral(install.agent_token)}${shellMirror}`;
    }
    if (installPlatform === "freebsd") {
      return `fetch -qo - ${installer}/install-freebsd.sh | sudo sh -s -- -e ${shellLiteral(origin)} -t ${shellLiteral(install.agent_token)}${shellMirror}`;
    }
    return `curl -fsSL ${installer}/agent.sh | sudo sh -s -- -e ${shellLiteral(origin)} -t ${shellLiteral(install.agent_token)}${shellMirror}`;
  }, [install, installPlatform]);
  const serverDialog = useDialog<HTMLFormElement>(editing !== null, () => setEditing(null));
  const installDialog = useDialog<HTMLElement>(Boolean(installCommand), () => setInstall(null));

  const toggleSelected = (id: string) => setSelectedIds((current) => current.includes(id) ? current.filter((value) => value !== id) : [...current, id]);
  const allSelected = servers.length > 0 && selectedIds.length === servers.length;
  const updateForm = <K extends keyof ServerInput>(key: K, value: ServerInput[K]) => setForm((current) => ({ ...current, [key]: value }));
  const updateSettings = <K extends keyof Settings>(key: K, value: Settings[K]) => setSettings((current) => current ? settingPatch(current, key, value) : current);
  const updateThemeOption = (key: string, value: ThemeSettingValue) => setSettings((current) => current ? {
    ...current,
    theme_options: { ...current.theme_options, [key]: value },
  } : current);
  const selectTab = (next: AdminTab) => {
    const path = adminTabPaths[next];
    if (window.location.pathname !== path) window.history.pushState(null, "", path);
    setTab(next);
    setNotice("");
    setError("");
  };
  const siteLogoUrl = settings ? settings.logo_url : config.logo_url;

  const remoteVisibleServers = useMemo(() => {
    const keyword = remoteQuery.trim().toLowerCase();
    if (!keyword) return servers;
    return servers.filter((server) => `${server.name} ${server.region} ${server.group_name} ${server.last_ip}`.toLowerCase().includes(keyword));
  }, [remoteQuery, servers]);
  const remoteServerById = useMemo(() => new Map(servers.map((server) => [server.id, server])), [servers]);
  const remoteAllSelected = servers.length > 0 && remoteSelectedIds.length === servers.length;

  const toggleRemoteServer = (serverId: string) => {
    setRemoteSelectedIds((current) => current.includes(serverId)
      ? current.filter((id) => id !== serverId)
      : [...current, serverId]);
  };

  const refreshRemoteTasks = useCallback(async (quiet = false, includeCompleted = false) => {
    const snapshot = remoteTasksRef.current.filter((task) => includeCompleted || isRemoteTaskActive(task.status));
    if (!snapshot.length) return;
    const requestId = ++remoteTasksRequestRef.current;
    const responses = await Promise.allSettled(snapshot.map((task) => api.remoteTask(task.id)));
    if (requestId !== remoteTasksRequestRef.current) return;

    const updates = new Map<string, RemoteTask>();
    let failed = false;
    responses.forEach((response) => {
      if (response.status === "fulfilled") updates.set(response.value.id, response.value);
      else failed = true;
    });
    setRemoteTasks((current) => current.map((task) => updates.get(task.id) ?? task));
    if (failed && !quiet) setError("部分执行结果刷新失败");
  }, []);

  const createRemoteTask = async (event: FormEvent) => {
    event.preventDefault();
    if (!remoteSelectedIds.length) {
      setError("请选择至少一台服务器");
      return;
    }
    if (!remoteCommand.trim()) {
      setError("请输入命令");
      return;
    }
    if (twoFactorStatus === null) {
      setError("正在读取两步验证状态，请稍候");
      return;
    }
    if (!twoFactorStatus.enabled) {
      setError("请先在登录与安全中启用 TOTP 两步验证");
      return;
    }

    setBusy(true);
    setError("");
    try {
      const code = await verificationDialog.ask("确认执行远程命令", true);
      if (code === null) return;
      const command = remoteCommand;
      const data = await api.createRemoteTask({
        server_ids: remoteSelectedIds,
        command,
        totp_code: code,
      });
      const requestedAt = Math.floor(Date.now() / 1000);
      const tasks = data.tasks.map(({ server_id, task_id }) => ({
        id: task_id,
        server_id,
        command,
        status: "pending" as const,
        requested_by: "",
        requested_at: requestedAt,
        started_at: null,
        completed_at: null,
        result: "",
        exit_code: null,
      }));
      remoteTasksRequestRef.current += 1;
      remoteTasksRef.current = tasks;
      setRemoteTasks(tasks);
      setRemotePollingUntil(Date.now() + REMOTE_TASK_POLL_TIMEOUT_MS);
      setRemoteCommand("");
      setNotice("已提交命令，请查看各节点的执行结果");
      await refreshRemoteTasks(true, true);
    } catch (err) {
      setError(err instanceof Error && err.name === "TimeoutError"
        ? "下发请求超时，命令可能已提交，请先检查节点，避免重复执行"
        : err instanceof Error ? err.message : "下发命令失败");
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (!authenticated || tab !== "remote" || !remoteTasksActive || !remotePollingUntil) return;
    if (Date.now() >= remotePollingUntil) {
      setRemotePollingUntil(0);
      return;
    }
    let stopped = false;
    let running = false;
    let timer: number | undefined;
    const timeout = window.setTimeout(() => setRemotePollingUntil(0), remotePollingUntil - Date.now());

    const canPoll = () => !document.hidden && navigator.onLine !== false;
    const clearTimer = () => {
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
    };
    const schedule = () => {
      clearTimer();
      if (!stopped && canPoll()) {
        timer = window.setTimeout(run, REMOTE_TASK_POLL_INTERVAL_MS);
      }
    };
    const run = async () => {
      clearTimer();
      if (stopped || running || !canPoll()) return;
      if (Date.now() >= remotePollingUntil) {
        setRemotePollingUntil(0);
        return;
      }
      running = true;
      try {
        await refreshRemoteTasks(true);
      } finally {
        running = false;
        schedule();
      }
    };
    const resume = () => {
      clearTimer();
      if (!stopped && canPoll()) void run();
    };
    const pause = () => clearTimer();
    const handleVisibility = () => {
      if (document.hidden) pause();
      else resume();
    };

    document.addEventListener("visibilitychange", handleVisibility);
    window.addEventListener("online", resume);
    window.addEventListener("offline", pause);
    schedule();
    return () => {
      stopped = true;
      clearTimer();
      window.clearTimeout(timeout);
      document.removeEventListener("visibilitychange", handleVisibility);
      window.removeEventListener("online", resume);
      window.removeEventListener("offline", pause);
    };
  }, [authenticated, refreshRemoteTasks, remoteTasksActive, remotePollingUntil, tab]);

  if (!authChecked) {
    return <div className={`admin-page ${dark ? "admin-dark" : ""}`} aria-busy="true" />;
  }

  return (
    <div className={`admin-page ${dark ? "admin-dark" : ""}`}>
      {authenticated ? (error ? <div className="admin-toast error" role="alert" aria-live="assertive"><CircleAlert aria-hidden="true" /><span>{error}</span></div>
        : notice ? <div className="admin-toast" role="status" aria-live="polite"><CircleCheck aria-hidden="true" /><span>{notice}</span></div> : null) : null}
      {!authenticated ? <div className="admin-login-stage">
        <button className="admin-login-theme" type="button" onClick={onToggleTheme} title={dark ? "切换浅色主题" : "切换深色主题"}>{dark ? <Sun size={15} /> : <Moon size={15} />}</button>
        <form className="login-form glass-panel" onSubmit={login}>
          <SiteLogo src={siteLogoUrl} alt="" width="48" height="48" />
          <div className="login-copy"><h1>管理员登录</h1><p>{config.site_name}</p></div>
          <label><span>用户名</span><input autoFocus type="text" autoComplete="username" value={username} onChange={(event) => setUsername(event.target.value)} required aria-invalid={error ? true : undefined} aria-describedby={error ? "login-error" : undefined} /></label>
          <label><span>密码</span><input type="password" autoComplete="current-password" value={password} onChange={(event) => setPassword(event.target.value)} required aria-invalid={error ? true : undefined} aria-describedby={error ? "login-error" : undefined} /></label>
          {loginTotpRequired ? <label><span>两步验证码</span><input autoFocus inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" maxLength={6} value={loginTotpCode} onChange={(event) => setLoginTotpCode(event.target.value.replace(/\D/g, "").slice(0, 6))} required aria-invalid={error ? true : undefined} aria-describedby={error ? "login-error" : undefined} /></label> : null}
          {config.turnstile_login_enabled ? <div className="login-turnstile"><TurnstileWidget siteKey={config.turnstile_site_key} action="admin_login" theme={dark ? "dark" : "light"} resetKey={turnstileReset} onVerify={setTurnstileToken} onError={setError} /></div> : null}
          {error ? <p className="login-error" id="login-error" role="alert"><CircleAlert size={15} aria-hidden="true" />{error}</p> : null}
          <button className="primary-btn login-submit" disabled={busy || (loginTotpRequired && !/^\d{6}$/.test(loginTotpCode)) || (config.turnstile_login_enabled && !turnstileToken)} type="submit"><KeyRound size={15} />{busy ? "验证中" : "登录"}</button>
        </form>
      </div> : <section className="admin-shell" aria-label="管理面板">
        <header className="admin-topbar">
          <div className="admin-brand">
            <SiteLogo src={siteLogoUrl} alt="" width="36" height="36" />
            <strong>{config.site_name}</strong>
          </div>
          <div className="admin-topbar-actions">
            <button type="button" onClick={onToggleTheme} title={dark ? "切换浅色主题" : "切换深色主题"} aria-label={dark ? "切换浅色主题" : "切换深色主题"}>{dark ? <Sun size={15} /> : <Moon size={15} />}</button>
            <a className="admin-home-link" href="/" target="_blank" rel="noopener noreferrer" title="主页" aria-label="主页"><ArrowLeft size={15} /></a>
            <button type="button" onClick={() => void logout()} title="退出" aria-label="退出"><LogOut size={15} /></button>
          </div>
        </header>

          <div className="admin-body">
            <aside className="admin-sidebar">
              <nav className="admin-tabs" aria-label="管理导航">
                {adminNavigation.map((item) => {
                  const Icon = item.icon;
                  return <a key={item.tab} href={adminTabPaths[item.tab]} className={tab === item.tab ? "active" : ""} aria-current={tab === item.tab ? "page" : undefined} onClick={(event) => {
                    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                    event.preventDefault();
                    selectTab(item.tab);
                  }}><Icon size={17} />{item.label}</a>;
                })}
              </nav>
            </aside>
            <div className="admin-content">
              <header className="admin-content-header"><h1>{adminPages[tab].title}</h1><p>{adminPages[tab].description}</p></header>
              {tab === "servers" ? (
                <div className="admin-section">
                  <div className="section-head"><div><h3>监控节点</h3><span>{servers.length} 个节点 · 可拖动上下排序</span></div><div className="section-actions"><button className="primary-btn compact" onClick={() => openEditor()}><Plus size={15} />添加</button></div></div>
                  <div className="batch-toolbar"><label className="select-all"><Checkbox checked={allSelected} onChange={() => setSelectedIds(allSelected ? [] : servers.map((server) => server.id))} />全选</label>{selectedIds.length ? <button className="danger-btn compact" onClick={() => void removeSelected()}><Trash2 size={15} />删除选中 ({selectedIds.length})</button> : <span>批量操作</span>}</div>
                  <div className="server-list">
                    {servers.map((server, index) => (
                      <div className={`server-row ${draggingId === server.id ? "dragging" : ""}`} key={server.id} onDragOver={(event) => { event.preventDefault(); event.dataTransfer.dropEffect = "move"; }} onDrop={(event) => void dropServer(event, server.id)}>
                        <button type="button" className="drag-handle" draggable onDragStart={(event) => startDrag(event, server.id)} onDragEnd={() => setDraggingId("")} title={`拖动排序：${server.name}`}><GripVertical size={15} /></button>
                        <Checkbox checked={selectedIds.includes(server.id)} onChange={() => toggleSelected(server.id)} ariaLabel={`选择 ${server.name}`} />
                        <div className="server-name"><div className="server-name-main"><Flag region={server.region} size={17} /><strong>{server.name}</strong></div><ServerIpMeta server={server} agentVersion={server.agent_version} onCopy={(value) => void copyServerIp(value)} /></div>
                        <div className="row-actions"><button className="icon-btn" disabled={index === 0} onClick={() => void move(index, -1)} title="上移"><ChevronUp size={15} /></button><button className="icon-btn" disabled={index === servers.length - 1} onClick={() => void move(index, 1)} title="下移"><ChevronDown size={15} /></button><button className="icon-btn" disabled={busy} onClick={() => void showInstallCommand(server)} title="下载 Agent"><Download size={15} /></button><button className="icon-btn" onClick={() => openEditor(server)} title="编辑节点"><Pencil size={15} /></button><button className="icon-btn danger" onClick={() => void remove(server)} title="删除节点"><Trash2 size={15} /></button></div>
                      </div>
                    ))}
                    {!servers.length && !busy ? <div className="list-empty">暂无节点</div> : null}
                  </div>
                </div>
              ) : tab === "latency" ? (
                <LatencyManager servers={servers} onError={setError} onNotice={setNotice} />
              ) : tab === "themes" && settings ? (
                <div className="theme-store-page">
                  <section className="admin-section">
                    <div className="section-head"><h3>主题列表</h3></div>
                    <div className="theme-list">
                      {themes.map((theme) => {
                        const uploaded = theme.url.startsWith("upload:");
                        return <article className={`theme-row ${theme.active ? "active" : ""}`} key={theme.id}>
                        <div className="theme-row-main">
                          <div className="theme-row-title"><strong>{theme.name}</strong><span className={`theme-badge ${theme.builtin ? "builtin" : uploaded ? "upload" : "remote"}`}>{theme.builtin ? "默认主题" : uploaded ? "上传安装" : "GitHub Release"}</span>{theme.version ? <span className="theme-badge version">v{theme.version}</span> : null}</div>
                          {!theme.builtin ? <p title={theme.description || undefined}>{theme.description || "暂无主题说明"}</p> : null}
                          {!theme.builtin && uploaded ? <span className="theme-upload-source"><Upload size={13} /><span>{theme.url.slice("upload:".length)}</span></span> : null}
                          {!theme.builtin && !uploaded ? <a href={theme.url} target="_blank" rel="noreferrer"><span>{theme.url}</span><ExternalLink size={13} /></a> : null}
                        </div>
                        <div className="theme-row-actions">
                          {!theme.builtin ? <button type="button" className="secondary-btn compact" disabled={busy} onClick={() => void previewTheme(theme)}><Eye size={15} />预览</button> : null}
                          <button type="button" className={theme.active ? "theme-active-btn" : "primary-btn compact"} disabled={busy || theme.active} onClick={() => void activateTheme(theme)}>{theme.active ? <><Check size={15} />使用中</> : "启用"}</button>
                          {!theme.builtin ? <button type="button" className="icon-btn danger" disabled={busy} title="删除主题" onClick={() => void removeTheme(theme)}><Trash2 size={15} /></button> : null}
                        </div>
                      </article>;
                      })}
                    </div>
                  </section>
                  <form className="admin-section theme-add-form" onSubmit={addTheme}>
                    <div className="section-head"><div><h3>安装主题</h3><span>主题包含可执行前端代码，只安装可信来源；安装后不依赖运行时远程资源。</span></div></div>
                    <div className="segmented theme-source-tabs" role="group" aria-label="主题安装来源">
                      <button type="button" className={themeSourceMode === "repository" ? "active" : ""} aria-pressed={themeSourceMode === "repository"} onClick={() => setThemeSourceMode("repository")}>GitHub 仓库</button>
                      <button type="button" className={themeSourceMode === "upload" ? "active" : ""} aria-pressed={themeSourceMode === "upload"} onClick={() => setThemeSourceMode("upload")}>上传</button>
                    </div>
                    <p className="settings-hint">{themeSourceMode === "repository" ? "填写仓库主页地址，NodeFlare 会下载 latest Release 中的第一个 ZIP 文件。" : "ZIP 根目录需包含 index.html，也支持外层只有一个目录的打包方式；最大 32 MiB。"}</p>
                    <div className="form-grid"><label><span>主题名称</span><input required maxLength={80} value={themeName} onChange={(event) => setThemeName(event.target.value)} placeholder="例如：Ocean" /></label>{themeSourceMode === "repository" ? <label><span>GitHub 仓库</span><input required type="url" maxLength={2048} value={themeUrl} onChange={(event) => setThemeUrl(event.target.value)} placeholder="https://github.com/user/theme" /></label> : <label className="theme-file-field"><span>文件</span><input ref={themeFileInputRef} required type="file" accept=".zip,application/zip" onChange={(event) => setThemeFile(event.target.files?.[0] ?? null)} /><small>{themeFile ? `${themeFile.name} · ${(themeFile.size / 1024 / 1024).toFixed(2)} MiB` : "请选择 .zip 文件"}</small></label>}</div>
                    <label><span>主题说明（可选）</span><textarea rows={2} maxLength={300} value={themeDescription} onChange={(event) => setThemeDescription(event.target.value)} placeholder="简短描述主题风格和来源" /></label>
                    <div className="form-actions"><button className="primary-btn" disabled={busy || (themeSourceMode === "upload" && !themeFile)}>{themeSourceMode === "upload" ? <Upload size={15} /> : <Download size={15} />}{busy ? "安装中" : "安装主题"}</button></div>
                  </form>
                </div>
              ) : settings && (tab === "appearance" || tab === "themeSettings" || tab === "alerts" || tab === "security" || tab === "data") ? (
                <form className="settings-form" onSubmit={tab === "data" ? (event) => { event.preventDefault(); void migrateDatabase(); } : saveSite}>
                  {tab === "appearance" ? <>
                    <div className="section-title"><Eye size={15} />外观与展示</div>
                    <div className="form-grid"><label><span>站点名称</span><input required value={settings.site_name} onChange={(event) => updateSettings("site_name", event.target.value)} /></label><label><span>站点描述</span><input value={settings.site_description} onChange={(event) => updateSettings("site_description", event.target.value)} /></label></div>
                    <label><span>站点公告</span><textarea rows={3} maxLength={1000} value={settings.site_announcement} onChange={(event) => updateSettings("site_announcement", event.target.value)} /></label>
                    <div className="form-grid"><label><span>界面语言</span><select value={settings.locale} onChange={(event) => updateSettings("locale", event.target.value as Settings["locale"])}><option value="zh-CN">简体中文</option><option value="en">English</option></select></label><label><span>站点 Logo / 浏览器图标地址</span><input type="url" maxLength={1000} value={settings.logo_url} onChange={(event) => updateSettings("logo_url", event.target.value)} placeholder="https://example.com/logo.svg" /></label></div>
                    <div className="form-grid"><label><span>离线判定（秒）</span><input type="number" min="30" max="3600" value={settings.offline_threshold_seconds} onChange={(event) => updateSettings("offline_threshold_seconds", Number(event.target.value))} /></label><label><span>历史保留（天）</span><input type="number" min="1" max="30" value={settings.history_retention_days} onChange={(event) => updateSettings("history_retention_days", Number(event.target.value))} /></label></div>
                  </> : null}

                  {tab === "themeSettings" ? <>
                    <div className="section-title"><SlidersHorizontal size={15} />通用主题设置</div>
                    <div className="form-grid"><label><span>默认主题</span><select value={settings.default_theme} onChange={(event) => updateSettings("default_theme", event.target.value as Settings["default_theme"])}><option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option></select></label><label><span>背景图地址</span><input type="text" maxLength={1000} value={settings.background_url} onChange={(event) => updateSettings("background_url", event.target.value)} placeholder="https://example.com/light.webp | https://example.com/dark.webp" /></label></div>
                    <p className="settings-hint">仅支持 HTTPS 地址；浅色和深色背景可用 | 分隔。</p>
                    <div className="section-subtitle">当前主题选项</div><div className="theme-option-grid"><Toggle label="公开仪表盘" checked={settings.public_dashboard} onChange={(value) => updateSettings("public_dashboard", value)} />{themeSettingsSchema?.settings.map((field) => <ThemeOption key={field.key} field={field} value={settings.theme_options[field.key] ?? field.default} onChange={(value) => updateThemeOption(field.key, value)} />)}</div>
                    <div className="section-subtitle">公开界面元素</div>
                    <div className="settings-toggles"><Toggle label="显示搜索" checked={settings.show_search} onChange={(value) => updateSettings("show_search", value)} /><Toggle label="显示分组" checked={settings.show_groups} onChange={(value) => updateSettings("show_groups", value)} /><Toggle label="总览统计" checked={settings.show_stats} onChange={(value) => updateSettings("show_stats", value)} /><Toggle label="资产统计" checked={settings.show_assets} onChange={(value) => updateSettings("show_assets", value)} /><Toggle label="累计流量" checked={settings.show_traffic} onChange={(value) => updateSettings("show_traffic", value)} /><Toggle label="实时网速" checked={settings.show_speed} onChange={(value) => updateSettings("show_speed", value)} /><Toggle label="价格信息" checked={settings.show_price} onChange={(value) => updateSettings("show_price", value)} /><Toggle label="到期信息" checked={settings.show_expiry} onChange={(value) => updateSettings("show_expiry", value)} /><Toggle label="延迟与丢包" checked={settings.show_latency} onChange={(value) => updateSettings("show_latency", value)} /><Toggle label="在线时长" checked={settings.show_uptime} onChange={(value) => updateSettings("show_uptime", value)} /></div>
                  </> : null}

                  {tab === "alerts" ? <>
                    <div className="section-title"><AlertTriangle size={15} />通知与告警</div>
                    <Toggle label="启用通知与告警" checked={settings.notification_enabled} onChange={(value) => updateSettings("notification_enabled", value)} />
                    <div className="form-grid three"><label><span>离线告警延迟（分钟）</span><input type="number" min="2" max="1440" value={settings.offline_alert_minutes} onChange={(event) => updateSettings("offline_alert_minutes", Number(event.target.value))} /></label><label><span>到期提醒（天）</span><input type="number" min="0" max="365" value={settings.expiry_alert_days} onChange={(event) => updateSettings("expiry_alert_days", Number(event.target.value))} /></label><label><span>流量提醒起始阈值（%）</span><input type="number" min="50" max="100" value={settings.traffic_alert_percentage} onChange={(event) => updateSettings("traffic_alert_percentage", Number(event.target.value))} /></label></div>
                    <p className="settings-hint">流量达到起始阈值后，每增加 5 个百分点生成一次事件，最多提醒到 100%。</p>
                    <TelegramSettings onError={setError} onNotice={setNotice} />
                    <AlertRuleManager servers={servers} onError={setError} onNotice={setNotice} />
                  </> : null}

                  {tab === "security" ? <>
                    <div className="section-title"><ShieldCheck size={15} />账号与 Cloudflare 防护</div>
                    <div className="form-grid three"><label><span>管理员用户名</span><input autoComplete="username" value={settings.admin_username} onChange={(event) => updateSettings("admin_username", event.target.value)} /></label><label><span>新密码（留空不修改）</span><PasswordInput autoComplete="new-password" minLength={8} maxLength={128} value={settings.new_password || ""} onChange={(event) => { updateSettings("new_password", event.target.value); setNewPasswordConfirmation(""); }} placeholder="至少 8 个字符" /></label><label><span>确认新密码</span><PasswordInput autoComplete="new-password" minLength={8} maxLength={128} value={newPasswordConfirmation} onChange={(event) => setNewPasswordConfirmation(event.target.value)} placeholder="再次输入新密码" /></label></div>
                    <div className="two-factor-panel">
                      <div className="two-factor-head"><div><div className="section-subtitle">TOTP 两步验证</div><p className="settings-hint">启用后管理员登录和每次远程执行都必须提交验证器生成的 6 位动态码。</p></div><span className={`two-factor-status ${twoFactorStatus?.enabled ? "enabled" : ""}`}>{twoFactorStatus?.enabled ? "已启用" : twoFactorStatus ? "未启用" : "读取中"}</span></div>
                      {twoFactorSetup ? <div className="two-factor-setup">
                        <label><span>验证器密钥</span><div className="copy-field"><input readOnly value={twoFactorSetup.secret} onFocus={(event) => event.currentTarget.select()} onClick={(event) => event.currentTarget.select()} /><button type="button" className={`secondary-btn compact copy-secret-btn ${twoFactorSecretCopied ? "copied" : ""}`} aria-live="polite" onClick={() => void copyTwoFactorSecret()}>{twoFactorSecretCopied ? <Check size={14} /> : <Copy size={14} />}{twoFactorSecretCopied ? "已复制" : "复制密钥"}</button></div></label>
                        <p className="settings-hint">在 Google Authenticator、Aegis、2FAS 等验证器中手动输入该密钥，再填写当前 6 位验证码确认。</p>
                      </div> : null}
                      {twoFactorStatus?.enabled ? <div className="two-factor-actions"><button type="button" className="danger-btn two-factor-disable-btn" disabled={busy} onClick={() => void disableTwoFactor()}>禁用两步验证</button></div> : <div className="two-factor-actions">
                        {!twoFactorSetup ? <p className="settings-hint">{twoFactorStatus?.has_secret ? "已有未启用的密钥；重新生成后，旧密钥会失效。" : "尚未生成两步验证密钥。"}</p> : null}
                        <button type="button" className="secondary-btn two-factor-generate-btn" disabled={busy} onClick={() => void setupTwoFactor()}>{twoFactorStatus?.has_secret ? "重新生成密钥" : "生成密钥"}</button>
                        {twoFactorSetup ? <button type="button" className="primary-btn" disabled={busy} onClick={() => void enableTwoFactor()}>启用两步验证</button> : null}
                      </div>}
                    </div>
                    <div className="login-devices-panel">
                      <div className="login-devices-head"><div><div className="section-subtitle">登录设备</div><p className="settings-hint">踢下线后，对应设备的登录状态会立即失效。</p></div><button type="button" className="secondary-btn compact" disabled={Boolean(revokingSessionId)} onClick={() => void loadLoginSessions()}><RotateCw size={14} />刷新</button></div>
                      {!loginSessionsLoaded ? <p className="settings-hint">正在读取登录设备...</p> : loginSessions.length ? <div className="login-device-list">{loginSessions.map((session) => <div className="login-device-row" key={session.id}><span className="login-device-icon"><MonitorSmartphone size={18} /></span><div className="login-device-copy"><div className="login-device-title"><strong>{describeLoginDevice(session.user_agent)}</strong>{session.current ? <span className="login-device-current">当前设备</span> : null}</div><span>{session.ip_address || "未知 IP"}</span><small title={session.user_agent}>最近活动 {formatSessionTime(session.last_seen_at)} · 登录于 {formatSessionTime(session.created_at)} · 到期 {formatSessionTime(session.expires_at)}</small></div>{!session.current ? <button type="button" className="danger-btn compact login-device-revoke" disabled={Boolean(revokingSessionId)} onClick={() => void revokeLoginSession(session)}><LogOut size={14} />{revokingSessionId === session.id ? "下线中" : "踢下线"}</button> : null}</div>)}</div> : <p className="settings-hint">暂无有效登录设备。</p>}
                    </div>
                    <div className="form-grid"><Toggle label="保护公开仪表盘" checked={settings.turnstile_enabled} onChange={(value) => updateSettings("turnstile_enabled", value)} /><Toggle label="保护管理员登录" checked={settings.turnstile_login_enabled} onChange={(value) => updateSettings("turnstile_login_enabled", value)} /></div>
                    <div className="form-grid"><label><span>Turnstile Site Key</span><input autoComplete="off" type="password" value={settings.turnstile_site_key} onChange={(event) => updateSettings("turnstile_site_key", event.target.value)} /></label><label><span>Turnstile Secret Key</span><input autoComplete="off" type="password" value={settings.turnstile_secret_key} onChange={(event) => updateSettings("turnstile_secret_key", event.target.value)} /></label></div>
                  </> : null}

                  {tab === "data" ? <>
                    <div className="section-head database-section-head"><div><h3>数据库维护</h3><span>备份包含节点、设置、历史、主题文件、任务及安全配置，请妥善保管。</span></div><div className="section-actions"><button type="button" className="secondary-btn compact" disabled={busy || database?.restart_required} onClick={() => void exportDatabaseBackup()}><Download size={15} />导出备份</button><button type="button" className="secondary-btn compact" disabled={busy || database?.restart_required} onClick={() => databaseRestoreInputRef.current?.click()}><Upload size={15} />恢复备份</button><input ref={databaseRestoreInputRef} hidden type="file" accept=".zip,application/zip" onChange={(event) => { const file = event.currentTarget.files?.[0]; event.currentTarget.value = ""; if (file) void restoreDatabaseBackup(file); }} /></div></div>
                    <div className="database-storage"><div><span>数据库大小</span><strong>{database ? formatBytes(database.size_bytes) : "读取中..."}</strong>{database ? <small>{database.kind === "postgresql" ? "PostgreSQL" : "SQLite"}{database.reclaimable_bytes ? ` · 可回收 ${formatBytes(database.reclaimable_bytes)}` : ""}</small> : null}</div><button type="button" className="secondary-btn" disabled={busy || !database || database.restart_required} onClick={() => void reclaimDatabase()}><RotateCw size={15} />回收空间</button></div>
                    <div className="database-migration">
                      <div><div className="section-title"><ArrowRightLeft size={15} />数据库迁移</div><p className="settings-hint">将当前 {database?.kind === "postgresql" ? "PostgreSQL" : "SQLite"} 数据复制到 {database?.kind === "postgresql" ? "SQLite" : "PostgreSQL"}，完成后自动更新配置，重启服务后生效。</p></div>
                      {database?.restart_required ? <div className="database-migration-ready" role="status" aria-live="polite"><CircleCheck size={20} /><div><strong>迁移完成，业务写入已暂停</strong><span>{databaseMigrationResult ? `已复制 ${databaseMigrationResult.migrated_rows.toLocaleString()} 行，新数据库大小 ${formatBytes(databaseMigrationResult.size_bytes)}，请重启服务` : "配置已更新，重启后切换到新数据库并恢复写入"}</span></div><button type="button" className="primary-btn" disabled={busy} onClick={() => void restartAfterDatabaseMigration()}>{restarting ? <RotateCw className="spin" size={15} /> : <Power size={15} />}{restarting ? "正在重启" : "立即重启"}</button></div> : <div className="database-migration-form"><label><span>目标数据库 URL</span><input required type="password" autoComplete="off" maxLength={2048} spellCheck={false} value={databaseMigrationUrl} onChange={(event) => setDatabaseMigrationUrl(event.target.value)} placeholder={database?.kind === "postgresql" ? "sqlite://nodeflare-migrated.db" : "postgres://user:password@127.0.0.1:5432/nodeflare?sslmode=prefer"} /></label><button className="primary-btn" disabled={busy || !databaseMigrationUrl.trim()}><ArrowRightLeft size={15} />{busy ? "迁移中" : "开始迁移"}</button></div>}
                    </div>
                    <div className="usage-section">
                      <div className="usage-head"><div><div className="section-title"><Coins size={15} />每日汇率</div><p className="settings-hint">{exchangeRates ? `${exchangeRates.source} · ${exchangeRates.date || "等待首次更新"}${exchangeRates.stale ? " · 数据待更新" : ""}` : "正在读取汇率快照"}</p></div><div className="usage-head-actions">{exchangeRatesExpanded ? <button type="button" className="secondary-btn compact" disabled={busy || database?.restart_required} onClick={() => void refreshExchangeRates()}><RotateCw size={15} />立即更新</button> : null}<button type="button" className="secondary-btn compact usage-toggle" aria-expanded={exchangeRatesExpanded} onClick={() => setExchangeRatesExpanded((expanded) => !expanded)}>{exchangeRatesExpanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}{exchangeRatesExpanded ? "收起" : "展开"}</button></div></div>
                      {exchangeRatesExpanded ? exchangeRates ? <div className="usage-table-wrap"><table className="usage-table"><thead><tr><th>币种</th><th>1 CNY 可兑换</th></tr></thead><tbody>{ASSET_CURRENCIES.filter((currency) => currency !== "CNY").map((currency) => <tr key={currency}><th scope="row">{currency}</th><td>{exchangeRates.rates[currency]?.toLocaleString(undefined, { maximumFractionDigits: 6 }) ?? "--"}</td></tr>)}</tbody></table></div> : <div className="usage-empty">尚未读取</div> : null}
                    </div>
                  </> : null}

                  {tab !== "data" ? <div className="form-actions"><button className="primary-btn" disabled={busy}><Save size={15} />保存设置</button></div> : null}
                </form>
              ) : tab === "remote" ? (
                <div className="admin-section remote-section">
                  <form className="remote-execution-form" onSubmit={(event) => void createRemoteTask(event)}>
                    <label className="remote-command-field">
                      <span>执行命令</span>
                      <textarea autoFocus required rows={5} maxLength={16_384} spellCheck={false} value={remoteCommand} onChange={(event) => setRemoteCommand(event.target.value)} />
                      <small>多行内容按一个脚本执行，完成后返回输出，单条命令最长执行 10 分钟。离开页面不会终止已下发的命令。</small>
                    </label>

                    <div className="server-picker remote-server-picker">
                      <div className="server-picker-head"><strong>选择服务器</strong><span>已选 {remoteSelectedIds.length} / 共 {servers.length}</span><button type="button" onClick={() => setRemoteSelectedIds(remoteAllSelected ? [] : servers.map((server) => server.id))}>{remoteAllSelected ? "取消全选" : "全选"}</button></div>
                      <div className="server-picker-search"><Search size={16} /><input aria-label="搜索远程执行服务器" placeholder="搜索服务器" value={remoteQuery} onChange={(event) => setRemoteQuery(event.target.value)} /></div>
                      <div className="server-picker-list">
                        {remoteVisibleServers.map((server) => <label className="server-picker-row" key={server.id}><Checkbox checked={remoteSelectedIds.includes(server.id)} onChange={() => toggleRemoteServer(server.id)} ariaLabel={`选择 ${server.name}`} /><span><strong>{server.name}</strong><small>{server.group_name || "默认"}</small></span></label>)}
                        {!remoteVisibleServers.length ? <div className="server-picker-empty">{servers.length ? "没有匹配的服务器" : "暂无服务器"}</div> : null}
                      </div>
                    </div>

                    {twoFactorStatus === null ? <p className="settings-hint">正在确认 TOTP 两步验证状态…</p> : twoFactorStatus.enabled ? <div className="remote-confirm-row"><button className="primary-btn remote-submit" disabled={busy || !remoteSelectedIds.length || !remotePayloadReady}><Terminal size={15} />{busy ? "下发中" : `确认执行${remoteSelectedIds.length ? ` (${remoteSelectedIds.length})` : ""}`}</button></div> : <div className="remote-2fa-required"><ShieldCheck size={18} /><div><strong>远程执行需要 TOTP 两步验证</strong><span>启用后才能向 Agent 发送命令。</span></div><button type="button" className="secondary-btn compact" onClick={() => selectTab("security")}>前往启用</button></div>}
                  </form>

                  {remoteTasks.length > 0 ? <div className="remote-results">
                    <div className="section-head"><div><h3>执行结果</h3>{remoteTasksActive ? <span className="remote-auto-refresh">{remotePollingUntil ? <RotateCw size={12} /> : null}{remotePollingUntil ? "等待结果，每 2 秒自动刷新" : "自动刷新已暂停"}</span> : <span>本次命令已结束</span>}</div><button type="button" className="secondary-btn compact" disabled={busy} onClick={() => { setRemotePollingUntil(Date.now() + REMOTE_TASK_POLL_TIMEOUT_MS); void refreshRemoteTasks(false, true); }}><RotateCw size={14} />刷新结果</button></div>
                    {remoteTasksActive && !remotePollingUntil ? <p className="settings-hint" role="status">已等待 1 分钟，命令可能仍在执行。点击“刷新结果”可继续查询，暂停刷新不会停止命令。</p> : null}
                    <div className="remote-command-summary"><span>本次命令</span><code>{remoteTasks[0]?.command}</code></div>
                    <div className="task-list">
                      {remoteTasks.map((task) => {
                        const server = remoteServerById.get(task.server_id);
                        return <div key={task.id} className="task-item remote-result-item">
                          <div className="task-header"><strong className="remote-result-server">{server?.name ?? task.server_id}</strong><span className={`task-status ${task.status}`}>{remoteTaskStatusLabels[task.status]}</span><span className="task-time">{new Date(task.requested_at * 1000).toLocaleString()}</span></div>
                          {task.status === "success" || task.status === "failed" ? <pre className={`task-result ${task.status}`}>{task.result || "（命令没有输出）"}</pre> : <p className="task-progress">{task.status === "pending" ? "等待 Agent 确认接收；断线后不会自动重发命令。" : "Agent 已接收，正在等待执行结果…"}</p>}
                        </div>;
                      })}
                    </div>
                  </div> : null}
                </div>
              ) : tab === "about" ? (
                <div className="admin-section about-page">
                  <div className="about-brand"><SiteLogo alt="" width="52" height="52" /><div><strong>NodeFlare</strong><small>基于 Rust、Axum、SQLite / PostgreSQL 与 WebSocket 的服务器监控</small></div></div>
                  <div className="about-rows">
                    <div className="about-row"><span>版本</span><strong>v{VERSION}</strong></div>
                    <div className="about-row"><span>项目地址</span><a href="https://github.com/elysia62/NodeFlare" target="_blank" rel="noreferrer">github.com/elysia62/NodeFlare<ExternalLink size={13} /></a></div>
                    <div className="about-row"><span>开源协议</span><strong>MIT License</strong></div>
                  </div>
                </div>
              ) : null}
            </div>
          </div>
      </section>}

      {editing ? <div className="submodal-backdrop" role="presentation" onMouseDown={serverDialog.onBackdropMouseDown}><form ref={serverDialog.dialogRef} className="editor-modal glass-panel" role="dialog" aria-modal="true" aria-labelledby="server-editor-title" tabIndex={-1} onSubmit={saveServer}>
        <header><div><span className="eyebrow">节点配置</span><h3 id="server-editor-title">{editing === "new" ? "添加节点" : `编辑 · ${editing.name}`}</h3></div></header>
        <div className="form-grid"><label><span>名称</span><input autoFocus required value={form.name} onChange={(event) => updateForm("name", event.target.value)} /></label><label><span>地区代码</span><input maxLength={16} placeholder="CN / JP / DE" value={form.region} onChange={(event) => updateForm("region", event.target.value.toUpperCase())} /></label><label><span>分组</span><input value={form.group_name} onChange={(event) => updateForm("group_name", event.target.value)} /></label><label><span>标签</span><input placeholder="主力, 线路:BGP" value={form.tags} onChange={(event) => updateForm("tags", event.target.value)} /></label></div>
        <div className="form-grid three"><label><span>流量限额（0 不限）</span><input placeholder="如 100 G，不带单位按 GB；0 不限" {...sizeInputProps(trafficLimitText, setTrafficLimitText, (bytes) => updateForm("traffic_limit", bytes), form.traffic_limit)} /></label><label><span>流量口径</span><select value={form.traffic_limit_type} onChange={(event) => updateForm("traffic_limit_type", event.target.value as ServerInput["traffic_limit_type"])}><option value="sum">上下行合计</option><option value="max">取较大值</option><option value="min">取较小值</option><option value="up">仅上行</option><option value="down">仅下行</option></select></label><label><span>流量重置日</span><input min="1" max="31" type="number" value={form.reset_day} onChange={(event) => updateForm("reset_day", Number(event.target.value))} /></label></div>
        <div className="form-grid three"><label><span>价格（0 免费）</span><input type="number" min="0" max="1000000000" step="any" inputMode="decimal" value={priceText} onChange={(event) => updatePriceText(event.target.value)} onBlur={() => setPriceText(String(form.price))} /></label><label><span>币种</span><select value={form.currency} onChange={(event) => updateForm("currency", event.target.value)}>{ASSET_CURRENCIES.map((code) => <option key={code}>{code}</option>)}</select></label><label><span>计费周期</span><select value={String(form.billing_cycle)} onChange={(event) => updateForm("billing_cycle", Number(event.target.value))}>{BILLING_CYCLES.map((cycle) => <option key={cycle.days} value={cycle.days}>{cycle.label}</option>)}{BILLING_CYCLES.every((cycle) => cycle.days !== form.billing_cycle) ? <option value={form.billing_cycle}>{form.billing_cycle} 天</option> : null}</select></label></div>
        <div className="form-grid three"><label><span>到期日期</span><input type="date" value={formatDate(form.expires_at)} onChange={(event) => updateForm("expires_at", event.target.value ? Math.floor(new Date(`${event.target.value}T00:00:00Z`).getTime() / 1000) : null)} /></label><label><span>历史保存间隔（秒）</span><input min="15" max="3600" type="number" value={form.report_interval} onChange={(event) => updateForm("report_interval", Number(event.target.value))} /></label><label><span>实时采样间隔（秒）</span><input min="3" max="60" type="number" value={form.collect_interval} onChange={(event) => updateForm("collect_interval", Number(event.target.value))} /></label></div>
        <div className="form-grid"><label><span>统计网卡（逗号分隔，留空自动）</span><input value={form.network_interface} onChange={(event) => updateForm("network_interface", event.target.value)} placeholder="eth0,ens3" /></label><label><span>Agent 下载加速（可选）</span><input value={form.agent_mirror} onChange={(event) => updateForm("agent_mirror", event.target.value.trim())} placeholder="https://ghproxy.net" /></label><label><span>上行流量当前值</span><input placeholder="如 500 G" {...sizeInputProps(txCurrentText, setTxCurrentText, setTxCurrentBytes, txCurrentBytes)} /></label><label><span>下行流量当前值</span><input placeholder="如 500 G" {...sizeInputProps(rxCurrentText, setRxCurrentText, setRxCurrentBytes, rxCurrentBytes)} /></label></div>
        <div className="settings-toggles editor-toggles"><Toggle label={form.billing_cycle <= 0 ? "自动续费（一次性不适用）" : "自动续费"} checked={form.auto_renewal} onChange={(value) => updateForm("auto_renewal", value)} /><Toggle label="Agent 自动更新" checked={form.auto_update} onChange={(value) => updateForm("auto_update", value)} /><Toggle label="隐藏节点" checked={form.hidden} onChange={(value) => updateForm("hidden", value)} /><Toggle label="关闭离线告警" checked={form.offline_notify_disabled} onChange={(value) => updateForm("offline_notify_disabled", value)} /></div>
        <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => setEditing(null)}>取消</button><button className="primary-btn" disabled={busy}><Save size={15} />保存节点</button></div>
      </form></div> : null}

      {installCommand ? <div className="submodal-backdrop" role="presentation" onMouseDown={installDialog.onBackdropMouseDown}><section ref={installDialog.dialogRef} className="install-modal glass-panel" role="dialog" aria-modal="true" aria-labelledby="install-dialog-title" tabIndex={-1}><header><div><span className="eyebrow">Agent 部署</span><h3 id="install-dialog-title">下载 Agent</h3></div><div className="segmented install-platform" role="group" aria-label="Agent 平台"><button type="button" className={installPlatform === "linux" ? "active" : ""} aria-pressed={installPlatform === "linux"} onClick={() => setInstallPlatform("linux")}>Linux</button><button type="button" className={installPlatform === "windows" ? "active" : ""} aria-pressed={installPlatform === "windows"} onClick={() => setInstallPlatform("windows")}>Windows</button><button type="button" className={installPlatform === "macos" ? "active" : ""} aria-pressed={installPlatform === "macos"} onClick={() => setInstallPlatform("macos")}>macOS ARM</button><button type="button" className={installPlatform === "freebsd" ? "active" : ""} aria-pressed={installPlatform === "freebsd"} onClick={() => setInstallPlatform("freebsd")}>FreeBSD</button></div></header><div className="install-list"><pre>{installCommand}</pre></div><div className="form-actions"><button className="secondary-btn" type="button" onClick={() => setInstall(null)}>关闭</button><button className="primary-btn" type="button" onClick={() => void copyInstallCommand()}><Copy size={15} />复制</button></div></section></div> : null}
      {verificationDialog.dialog}
    </div>
  );
}

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (value: boolean) => void }) {
  return <label className="toggle-row"><span><b>{label}</b></span><Checkbox checked={checked} onChange={onChange} /></label>;
}

function ThemeOption({ field, value, onChange }: { field: ThemeSettingField; value: ThemeSettingValue | undefined; onChange: (value: ThemeSettingValue) => void }) {
  if (field.type === "toggle") {
    return <Toggle label={field.label} checked={typeof value === "boolean" ? value : false} onChange={onChange} />;
  }
  if (field.type === "textarea") {
    return <label className="theme-option"><span>{field.label}</span><textarea rows={3} maxLength={500} placeholder={field.placeholder} value={typeof value === "string" ? value : ""} onChange={(event) => onChange(event.target.value)} /></label>;
  }
  if (field.type === "select") {
    return <label className="theme-option"><span>{field.label}</span><select value={typeof value === "string" ? value : ""} onChange={(event) => onChange(event.target.value)}>{field.options?.map((option) => <option value={option.value} key={option.value}>{option.label}</option>)}</select></label>;
  }
  if (field.type === "number") {
    return <label className="theme-option"><span>{field.label}</span><input type="number" min={field.min} max={field.max} step={field.step} value={typeof value === "number" ? value : ""} onChange={(event) => onChange(Number(event.target.value))} /></label>;
  }
  return <label className={`theme-option ${field.type === "color" ? "theme-color-option" : ""}`}><span>{field.label}</span><input type={field.type} maxLength={field.type === "color" ? undefined : 500} placeholder={field.placeholder} value={typeof value === "string" ? value : field.type === "color" ? "#0f766e" : ""} onChange={(event) => onChange(event.target.value)} /></label>;
}
