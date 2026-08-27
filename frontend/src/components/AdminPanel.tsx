import {
  AlertTriangle,
  Info,
  ArrowLeft,
  ChevronDown,
  ChevronUp,
  CircleAlert,
  Cloud,
  CircleCheck,
  Coins,
  Copy,
  Database,
  Download,
  Eye,
  GripVertical,
  KeyRound,
  LogOut,
  Moon,
  Palette,
  Pencil,
  Plus,
  RadioTower,
  RotateCw,
  Save,
  ServerCog,
  ShieldCheck,
  SlidersHorizontal,
  Sun,
  Trash2,
  Check,
  ExternalLink,
} from "lucide-react";
import { DragEvent, FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { ADMIN_UNAUTHORIZED_EVENT, api, ApiError, getToken, setToken } from "../api";
import { isOnline } from "../format";
import { derivePassword } from "../password";
import { ASSET_CURRENCIES, type AdminServer, type CloudflareUsage, type Config, type DatabaseStats, type ExchangeRates, type ServerInput, type Settings, type Theme, type ThemeSettingField, type ThemeSettingsSchema, type ThemeSettingValue } from "../types";
import { Checkbox } from "./Checkbox";
import { TurnstileWidget } from "./TurnstileWidget";
import { useDialog } from "./useDialog";
import { SiteLogo } from "./SiteLogo";
import { LatencyManager } from "./LatencyManager";
import { AlertRuleManager } from "./AlertRuleManager";
import { TelegramSettings } from "./TelegramSettings";
import { Flag } from "./Flag";
import pkg from "../../package.json";

const VERSION = import.meta.env.VITE_NODEFLARE_VERSION || pkg.version;

type AdminTab = "servers" | "latency" | "appearance" | "themes" | "themeSettings" | "alerts" | "security" | "data" | "about";
type AgentPlatform = "linux" | "windows" | "macos" | "freebsd";

interface AgentInstallInfo {
  agent_token: string;
  agent_mirror: string;
}

const adminPages: Record<AdminTab, { title: string; description: string }> = {
  servers: { title: "服务器", description: "管理监控节点和运行参数" },
  latency: { title: "延迟检测", description: "配置分配给各服务器的 TCP 与 ICMP 延迟任务" },
  appearance: { title: "站点设置", description: "调整站点信息、公开内容和前台显示项目" },
  themes: { title: "主题商店", description: "选择内置主题或安装远程主题" },
  themeSettings: { title: "主题设置", description: "调整当前前端主题提供的显示选项" },
  alerts: { title: "通知", description: "配置 Telegram 通知和资源告警阈值" },
  security: { title: "登录与安全", description: "管理管理员账号和 Cloudflare Turnstile 防护" },
  data: { title: "监控数据库", description: "查看 D1 用量、汇率状态并维护历史数据" },
  about: { title: "关于", description: "版本信息与项目地址" },
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
  collect_interval: 1,
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

function formatDate(value: number | null) {
  return value ? new Date(value * 1000).toISOString().slice(0, 10) : "";
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

export function AdminPanel({
  config,
  dark,
  onToggleTheme,
  onClose,
  onChanged,
}: {
  config: Config;
  dark: boolean;
  onToggleTheme: () => void;
  onClose: () => void;
  onChanged: () => void;
}) {
  const [authenticated, setAuthenticated] = useState(!!getToken());
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [turnstileToken, setTurnstileToken] = useState("");
  const [turnstileReset, setTurnstileReset] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [tab, setTab] = useState<AdminTab>("servers");
  const [servers, setServers] = useState<AdminServer[]>([]);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [draggingId, setDraggingId] = useState("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [database, setDatabase] = useState<DatabaseStats | null>(null);
  const [cloudflareUsage, setCloudflareUsage] = useState<CloudflareUsage | null>(null);
  const [exchangeRates, setExchangeRates] = useState<ExchangeRates | null>(null);
  const [themes, setThemes] = useState<Theme[]>([]);
  const [themeName, setThemeName] = useState("");
  const [themeDescription, setThemeDescription] = useState("");
  const [themeUrl, setThemeUrl] = useState("");
  const [themeSettingsSchema, setThemeSettingsSchema] = useState<ThemeSettingsSchema | null>(null);
  const [editing, setEditing] = useState<AdminServer | "new" | null>(null);
  const [form, setForm] = useState<ServerInput>(emptyServer);
  const [install, setInstall] = useState<AgentInstallInfo | null>(null);
  const [rxCurrentGb, setRxCurrentGb] = useState("0");
  const [txCurrentGb, setTxCurrentGb] = useState("0");
  const [rxBaseBytes, setRxBaseBytes] = useState(0);
  const [txBaseBytes, setTxBaseBytes] = useState(0);
  const [installPlatform, setInstallPlatform] = useState<AgentPlatform>("linux");

  const load = useCallback(async () => {
    if (!getToken()) return;
    setBusy(true);
    setError("");
    try {
      const [serverResult, settingsResult, themesResult] = await Promise.all([api.adminServers(), api.settings(), api.themes()]);
      setServers(serverResult.servers);
      setThemes(themesResult.themes);
      setSettings(settingsResult);
      setAuthenticated(true);
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 401) {
        setToken("");
        setAuthenticated(false);
        setError("");
        return;
      }
      setError(reason instanceof Error ? reason.message : "加载失败");
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  useEffect(() => {
    const resetAuthentication = () => {
      setAuthenticated(false);
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

  async function login(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      const passwordDerived = await derivePassword(password, config.password_client_salt);
      const result = await api.login(username.trim(), password, passwordDerived, turnstileToken);
      setToken(result.token);
      setPassword("");
      setTurnstileToken("");
      setAuthenticated(true);
      await load();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "登录失败");
      setTurnstileToken("");
      setTurnstileReset((value) => value + 1);
    } finally { setBusy(false); }
  }

  function openEditor(server?: AdminServer) {
    setEditing(server ?? "new");
    setForm(server ? toInput(server) : { ...emptyServer });
    // 修正值输入展示“当前累计用量”（已上报 + 已修正），保存时换算回差值。
    const rxBase = server ? (server.net_rx_total ?? 0) - server.rx_correction : 0;
    const txBase = server ? (server.net_tx_total ?? 0) - server.tx_correction : 0;
    setRxCurrentGb(((server ? (server.net_rx_total ?? 0) : 0) / 1024 ** 3).toFixed(2));
    setTxCurrentGb(((server ? (server.net_tx_total ?? 0) : 0) / 1024 ** 3).toFixed(2));
    setRxBaseBytes(rxBase);
    setTxBaseBytes(txBase);
    setError("");
  }

  async function saveServer(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError("");
    const payload: ServerInput = {
      ...form,
      rx_correction: Math.round(((Number(rxCurrentGb) || 0) * 1024 ** 3) - rxBaseBytes),
      tx_correction: Math.round(((Number(txCurrentGb) || 0) * 1024 ** 3) - txBaseBytes),
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
      const { agent_token } = await api.serverToken(server.id);
      setInstall({ agent_token, agent_mirror: server.agent_mirror });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "读取 Agent Token 失败");
    } finally {
      setBusy(false);
    }
  }

  async function copyInstallCommand() {
    try {
      if (installCommand) await navigator.clipboard.writeText(installCommand);
    } catch {
      setError("复制失败，请手动选择命令复制");
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
    try {
      if (payload.new_password) {
        payload.new_password_derived = await derivePassword(payload.new_password, config.password_client_salt);
      } else {
        delete payload.new_password;
        delete payload.new_password_derived;
      }
      const result = await api.saveSettings(payload);
      if (result.token) setToken(result.token);
      setSettings(result.settings);
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
      setExchangeRates(rates);
    }
    catch (reason) { setError(reason instanceof Error ? reason.message : "读取数据库统计失败"); }
    finally { setBusy(false); }
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

  async function loadCloudflareUsage() {
    setBusy(true); setError("");
    try { setCloudflareUsage(await api.cloudflareUsage()); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "读取 Cloudflare 用量失败"); }
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
    setBusy(true); setError(""); setNotice("");
    try {
      await api.addTheme({ name: themeName.trim(), description: themeDescription.trim(), url: themeUrl.trim() });
      setThemeName(""); setThemeDescription(""); setThemeUrl("");
      await load();
      setNotice("主题已添加");
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

  async function clearHistory() {
    if (!window.confirm("确认清空全部历史指标？节点配置和最新状态不会删除。")) return;
    setBusy(true); setError("");
    try { await api.clearHistory(); setNotice("历史指标已清空"); await loadDatabase(); }
    catch (reason) { setError(reason instanceof Error ? reason.message : "清理历史失败"); }
    finally { setBusy(false); }
  }

  async function logout() {
    try { await api.logout(); }
    finally {
      setToken(""); setAuthenticated(false); setServers([]); setSettings(null); setSelectedIds([]);
    }
  }

  const installCommand = useMemo(() => {
    if (!install) return "";
    const origin = window.location.origin;
    const installer = "https://raw.githubusercontent.com/imengying/NodeFlare/main/agent";
    const mirror = install.agent_mirror.trim().replace(/\/+$/, "");
    const shellMirror = mirror ? ` -m ${shellLiteral(mirror)}` : "";
    const powershellMirror = mirror ? ` -Mirror ${powershellLiteral(mirror)}` : "";
    if (installPlatform === "windows") {
      return `Invoke-WebRequest -Uri "${installer}/install.ps1" -OutFile "$env:TEMP\\nodeflare-install.ps1"\n& "$env:TEMP\\nodeflare-install.ps1" -e ${powershellLiteral(origin)} -t ${powershellLiteral(install.agent_token)}${powershellMirror}`;
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
    setTab(next);
    setNotice("");
    setError("");
  };
  const siteLogoUrl = settings ? settings.logo_url : config.logo_url;

  return (
    <div className={`admin-page ${dark ? "admin-dark" : ""}`}>
      {error ? <div className="admin-toast error" role="alert" aria-live="assertive"><CircleAlert aria-hidden="true" /><span>{error}</span></div>
        : notice ? <div className="admin-toast" role="status" aria-live="polite"><CircleCheck aria-hidden="true" /><span>{notice}</span></div> : null}
      {!authenticated ? <div className="admin-login-stage">
        <button className="admin-back" type="button" onClick={onClose}><ArrowLeft size={14} />返回</button>
        <button className="admin-login-theme" type="button" onClick={onToggleTheme} title={dark ? "切换浅色主题" : "切换深色主题"}>{dark ? <Sun size={16} /> : <Moon size={16} />}</button>
        <form className="login-form glass-panel" onSubmit={login}>
          <SiteLogo src={siteLogoUrl} alt="" width="48" height="48" />
          <div className="login-copy"><h1>管理员登录</h1></div>
          <label><span>用户名</span><input autoFocus type="text" autoComplete="username" value={username} onChange={(event) => setUsername(event.target.value)} required /></label>
          <label><span>密码</span><input type="password" autoComplete="current-password" value={password} onChange={(event) => setPassword(event.target.value)} required /></label>
          {config.turnstile_login_enabled || config.turnstile_enabled ? <div className="login-turnstile"><TurnstileWidget siteKey={config.turnstile_site_key} action="admin-login" theme={dark ? "dark" : "light"} resetKey={turnstileReset} onVerify={setTurnstileToken} onError={setError} /></div> : null}
          <button className="primary-btn login-submit" disabled={busy || ((config.turnstile_login_enabled || config.turnstile_enabled) && !turnstileToken)} type="submit"><KeyRound size={16} />{busy ? "验证中" : "登录"}</button>
        </form>
      </div> : <section className="admin-shell" aria-label="管理面板">
        <header className="admin-topbar">
          <div className="admin-brand">
            <SiteLogo src={siteLogoUrl} alt="" width="36" height="36" />
            <strong>{config.site_name}</strong>
          </div>
          <div className="admin-topbar-actions">
            <button type="button" onClick={onToggleTheme} title={dark ? "切换浅色主题" : "切换深色主题"} aria-label={dark ? "切换浅色主题" : "切换深色主题"}>{dark ? <Sun size={17} /> : <Moon size={17} />}</button>
            <button type="button" onClick={onClose} title="主页" aria-label="主页"><ArrowLeft size={17} />主页</button>
            <button type="button" onClick={() => void logout()} title="退出" aria-label="退出"><LogOut size={17} />退出</button>
          </div>
        </header>

          <div className="admin-body">
            <aside className="admin-sidebar">
              <span className="admin-nav-label">管理</span>
              <nav className="admin-tabs">
                <button className={tab === "servers" ? "active" : ""} onClick={() => selectTab("servers")}><ServerCog size={19} />服务器</button>
                <button className={tab === "latency" ? "active" : ""} onClick={() => selectTab("latency")}><RadioTower size={19} />延迟检测</button>
                <button className={tab === "appearance" ? "active" : ""} onClick={() => selectTab("appearance")}><Eye size={19} />站点设置</button>
                <button className={tab === "themes" ? "active" : ""} onClick={() => selectTab("themes")}><Palette size={19} />主题商店</button>
                <button className={tab === "themeSettings" ? "active" : ""} onClick={() => { selectTab("themeSettings"); void loadThemeSettings(); }}><SlidersHorizontal size={19} />主题设置</button>
                <button className={tab === "alerts" ? "active" : ""} onClick={() => selectTab("alerts")}><AlertTriangle size={19} />通知</button>
                <button className={tab === "security" ? "active" : ""} onClick={() => selectTab("security")}><ShieldCheck size={19} />登录与安全</button>
                <button className={tab === "data" ? "active" : ""} onClick={() => { selectTab("data"); if (!database) void loadDatabase(); }}><Database size={19} />监控数据库</button>
                <button className={tab === "about" ? "active" : ""} onClick={() => selectTab("about")}><Info size={19} />关于</button>
              </nav>
            </aside>
            <div className="admin-content">
              <header className="admin-content-header"><h1>{adminPages[tab].title}</h1><p>{adminPages[tab].description}</p></header>
              {tab === "servers" ? (
                <div className="admin-section">
                  <div className="section-head"><div><h3>监控节点</h3><span>{servers.length} 个节点 · 可拖动上下排序</span></div><div className="section-actions"><button className="primary-btn compact" onClick={() => openEditor()}><Plus size={16} />添加</button></div></div>
                  <div className="batch-toolbar"><label className="select-all"><Checkbox checked={allSelected} onChange={() => setSelectedIds(allSelected ? [] : servers.map((server) => server.id))} />全选</label>{selectedIds.length ? <button className="danger-btn compact" onClick={() => void removeSelected()}><Trash2 size={14} />删除选中 ({selectedIds.length})</button> : <span>批量操作</span>}</div>
                  <div className="server-list">
                    {servers.map((server, index) => (
                      <div className={`server-row ${draggingId === server.id ? "dragging" : ""}`} key={server.id} onDragOver={(event) => { event.preventDefault(); event.dataTransfer.dropEffect = "move"; }} onDrop={(event) => void dropServer(event, server.id)}>
                        <button type="button" className="drag-handle" draggable onDragStart={(event) => startDrag(event, server.id)} onDragEnd={() => setDraggingId("")} title={`拖动排序：${server.name}`}><GripVertical size={15} /></button>
                        <Checkbox checked={selectedIds.includes(server.id)} onChange={() => toggleSelected(server.id)} ariaLabel={`选择 ${server.name}`} />
                        <span className={`status-dot ${settings && isOnline(server, settings.offline_threshold_seconds) ? "online" : ""}`} />
                        <div className="server-name"><div className="server-name-main"><Flag region={server.region} size={17} /><strong>{server.name}</strong><small>{server.last_ip || "尚未上报 IP"} · {server.agent_version ? `Agent v${server.agent_version}` : "Agent 未上报版本"}</small></div></div>
                        <div className="row-actions"><button className="icon-btn" disabled={index === 0} onClick={() => void move(index, -1)} title="上移"><ChevronUp size={15} /></button><button className="icon-btn" disabled={index === servers.length - 1} onClick={() => void move(index, 1)} title="下移"><ChevronDown size={15} /></button><button className="icon-btn" disabled={busy} onClick={() => void showInstallCommand(server)} title="显示安装命令"><Download size={15} /></button><button className="icon-btn" onClick={() => openEditor(server)} title="编辑节点"><Pencil size={15} /></button><button className="icon-btn danger" onClick={() => void remove(server)} title="删除节点"><Trash2 size={15} /></button></div>
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
                      {themes.map((theme) => <article className={`theme-row ${theme.active ? "active" : ""}`} key={theme.id}>
                        <div className="theme-row-main">
                          <div className="theme-row-title"><strong>{theme.name}</strong><span className={`theme-badge ${theme.builtin ? "builtin" : "remote"}`}>{theme.builtin ? "默认主题" : "远程主题"}</span>{theme.version ? <span className="theme-badge version">v{theme.version}</span> : null}</div>
                          {!theme.builtin ? <p title={theme.description || undefined}>{theme.description || "暂无主题说明"}</p> : null}
                          {!theme.builtin ? <a href={theme.url} target="_blank" rel="noreferrer"><span>{theme.url}</span><ExternalLink size={13} /></a> : null}
                        </div>
                        <div className="theme-row-actions">
                          {!theme.builtin ? <button type="button" className="secondary-btn compact" disabled={busy} onClick={() => void previewTheme(theme)}><Eye size={14} />预览</button> : null}
                          <button type="button" className={theme.active ? "theme-active-btn" : "primary-btn compact"} disabled={busy || theme.active} onClick={() => void activateTheme(theme)}>{theme.active ? <><Check size={14} />使用中</> : "启用"}</button>
                          {!theme.builtin ? <button type="button" className="icon-btn danger" disabled={busy} title="删除主题" onClick={() => void removeTheme(theme)}><Trash2 size={15} /></button> : null}
                        </div>
                      </article>)}
                    </div>
                  </section>
                  <form className="admin-section theme-add-form" onSubmit={addTheme}>
                    <div className="section-head"><h3>添加远程主题</h3></div>
                    <p className="settings-hint">仅支持 GitHub 地址</p>
                    <div className="form-grid"><label><span>主题名称</span><input required maxLength={80} value={themeName} onChange={(event) => setThemeName(event.target.value)} placeholder="例如：Ocean" /></label><label><span>主题 URL</span><input required type="url" maxLength={2048} value={themeUrl} onChange={(event) => setThemeUrl(event.target.value)} placeholder="https://github.com/user/theme" /></label></div>
                    <label><span>主题说明（可选）</span><textarea rows={2} maxLength={300} value={themeDescription} onChange={(event) => setThemeDescription(event.target.value)} placeholder="简短描述主题风格和来源" /></label>
                    <div className="form-actions"><button className="primary-btn" disabled={busy}><Plus size={16} />添加主题</button></div>
                  </form>
                </div>
              ) : settings && tab !== "about" ? (
                <form className="settings-form" onSubmit={saveSite}>
                  {tab === "appearance" ? <>
                    <div className="section-title"><Eye size={15} />外观与展示</div>
                    <div className="form-grid"><label><span>站点名称</span><input required value={settings.site_name} onChange={(event) => updateSettings("site_name", event.target.value)} /></label><label><span>站点描述</span><input value={settings.site_description} onChange={(event) => updateSettings("site_description", event.target.value)} /></label></div>
                    <label><span>站点公告</span><textarea rows={3} maxLength={1000} value={settings.site_announcement} onChange={(event) => updateSettings("site_announcement", event.target.value)} /></label>
                    <div className="form-grid three"><label><span>界面语言</span><select value={settings.locale} onChange={(event) => updateSettings("locale", event.target.value as Settings["locale"])}><option value="zh-CN">简体中文</option><option value="en">English</option></select></label><label><span>站点 Logo 地址</span><input type="url" maxLength={1000} value={settings.logo_url} onChange={(event) => updateSettings("logo_url", event.target.value)} placeholder="https://example.com/logo.svg" /></label><label><span>浏览器图标地址</span><input type="url" maxLength={1000} value={settings.favicon_url} onChange={(event) => updateSettings("favicon_url", event.target.value)} placeholder="https://example.com/favicon.png" /></label></div>
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
                    <div className="form-grid"><label><span>管理员用户名</span><input autoComplete="username" value={settings.admin_username} onChange={(event) => updateSettings("admin_username", event.target.value)} /></label><label><span>新密码（留空不修改）</span><input autoComplete="new-password" type="password" value={settings.new_password || ""} onChange={(event) => updateSettings("new_password", event.target.value)} placeholder="至少 8 个字符" /></label></div>
                    <div className="form-grid"><Toggle label="保护公开仪表盘" checked={settings.turnstile_enabled} onChange={(value) => updateSettings("turnstile_enabled", value)} /><Toggle label="保护管理员登录" checked={settings.turnstile_login_enabled} onChange={(value) => updateSettings("turnstile_login_enabled", value)} /></div>
                    <div className="form-grid"><label><span>Turnstile Site Key</span><input autoComplete="off" type="password" value={settings.turnstile_site_key} onChange={(event) => updateSettings("turnstile_site_key", event.target.value)} /></label><label><span>Turnstile Secret Key</span><input autoComplete="off" type="password" value={settings.turnstile_secret_key} onChange={(event) => updateSettings("turnstile_secret_key", event.target.value)} /></label></div>
                    <p className="settings-hint">留空则读取后台配置。</p>
                  </> : null}

                  {tab === "data" ? <>
                    <div className="section-title"><Database size={15} />D1 数据维护</div>
                    <div className="data-stat-grid">{database ? <><DataStat label="节点" value={database.server_count} /><DataStat label="在线" value={database.online_count} /><DataStat label="历史行数" value={database.history_rows.toLocaleString()} /></> : <p className="settings-hint">正在读取数据库统计...</p>}</div>
                    <div className="usage-section">
                      <div className="usage-head"><div><div className="section-title"><Coins size={15} />每日汇率</div><p className="settings-hint">{exchangeRates ? `${exchangeRates.source} · ${exchangeRates.date || "等待首次更新"}${exchangeRates.stale ? " · 数据待更新" : ""}` : "正在读取 D1 汇率快照"}</p></div><button type="button" className="secondary-btn compact" disabled={busy} onClick={() => void refreshExchangeRates()}><RotateCw size={14} />立即更新</button></div>
                      {exchangeRates ? <div className="usage-table-wrap"><table className="usage-table"><thead><tr><th>币种</th><th>1 CNY 可兑换</th></tr></thead><tbody>{ASSET_CURRENCIES.map((currency) => <tr key={currency}><th scope="row">{currency}</th><td>{exchangeRates.rates[currency]?.toLocaleString(undefined, { maximumFractionDigits: 6 }) ?? "--"}</td></tr>)}</tbody></table></div> : <div className="usage-empty">尚未读取</div>}
                    </div>
                    <div className="usage-section">
                      <div className="usage-head"><div><div className="section-title"><Cloud size={15} />Cloudflare 用量</div><p className="settings-hint">统计周期使用 UTC</p></div><button type="button" className="secondary-btn compact" disabled={busy} onClick={() => void loadCloudflareUsage()}><RotateCw size={14} />{cloudflareUsage ? "刷新用量" : "查询用量"}</button></div>
                      <div className="form-grid"><label><span>Cloudflare Account ID</span><input autoComplete="off" type="password" maxLength={32} value={settings.cloudflare_account_id} onChange={(event) => updateSettings("cloudflare_account_id", event.target.value)} placeholder="32 位账户 ID" /></label><label><span>Cloudflare API Token</span><input autoComplete="off" type="password" value={settings.cloudflare_api_token} onChange={(event) => updateSettings("cloudflare_api_token", event.target.value)} placeholder="Account Analytics: Read" /></label></div>
                      <div className="usage-config-actions"><p className="settings-hint">Token 需要账户级 Account Analytics: Read 权限，并授权对应账户；留空读取 Worker Secret。</p><button type="submit" className="primary-btn compact" disabled={busy}><Save size={14} />保存用量配置</button></div>
                      {cloudflareUsage ? <><div className="usage-table-wrap"><table className="usage-table cloudflare-usage-table"><thead><tr><th>周期</th><th>D1 读取</th><th>D1 写入</th><th>Workers 请求</th><th>DO 请求（估算）</th><th>DO 时长 (GB-s)</th></tr></thead><tbody><UsageRow label="今日" usage={cloudflareUsage.today} /><UsageRow label="昨日" usage={cloudflareUsage.yesterday} /></tbody></table></div><div className="usage-do-breakdown"><UsageDoBreakdown label="今日" usage={cloudflareUsage.today} /><UsageDoBreakdown label="昨日" usage={cloudflareUsage.yesterday} /></div></> : <div className="usage-empty">尚未读取</div>}
                    </div>
                    <div className="data-actions"><button type="button" className="secondary-btn" onClick={() => void loadDatabase()}>刷新统计</button><button type="button" className="danger-btn" onClick={() => void clearHistory()}><Trash2 size={15} />清空历史指标</button></div>
                    <p className="settings-hint">清空历史不会删除节点、密钥或最新状态。</p>
                  </> : null}

                  {tab !== "data" ? <div className="form-actions"><button className="primary-btn" disabled={busy}><Save size={16} />保存{tab === "security" ? "账号与安全设置" : "设置"}</button></div> : null}
                </form>
              ) : tab === "about" ? (
                <div className="admin-section about-page">
                  <div className="about-brand"><SiteLogo alt="" width="52" height="52" /><div><strong>NodeFlare</strong><small>基于 Rust、WebAssembly 和 Cloudflare Workers 的服务器监控</small></div></div>
                  <div className="about-rows">
                    <div className="about-row"><span>版本</span><strong>v{VERSION}</strong></div>
                    <div className="about-row"><span>项目地址</span><a href="https://github.com/imengying/NodeFlare" target="_blank" rel="noreferrer">github.com/imengying/NodeFlare<ExternalLink size={13} /></a></div>
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
        <div className="form-grid three"><label><span>流量限额（GB）</span><input min="0" type="number" value={Math.round(form.traffic_limit / 1024 ** 3)} onChange={(event) => updateForm("traffic_limit", Number(event.target.value) * 1024 ** 3)} /></label><label><span>流量口径</span><select value={form.traffic_limit_type} onChange={(event) => updateForm("traffic_limit_type", event.target.value as ServerInput["traffic_limit_type"])}><option value="sum">上下行合计</option><option value="max">取较大值</option><option value="min">取较小值</option><option value="up">仅上行</option><option value="down">仅下行</option></select></label><label><span>流量重置日</span><input min="1" max="31" type="number" value={form.reset_day} onChange={(event) => updateForm("reset_day", Number(event.target.value))} /></label></div>
        <div className="form-grid three"><label><span>价格（0 隐藏，-1 免费）</span><input min="-1" step="0.01" type="number" value={form.price} onChange={(event) => updateForm("price", Number(event.target.value))} /></label><label><span>币种</span><select value={form.currency} onChange={(event) => updateForm("currency", event.target.value)}>{ASSET_CURRENCIES.map((code) => <option key={code}>{code}</option>)}</select></label><label><span>计费周期（天）</span><input min="1" max="3650" type="number" value={form.billing_cycle} onChange={(event) => updateForm("billing_cycle", Number(event.target.value))} /></label></div>
        <div className="form-grid three"><label><span>到期日期</span><input type="date" value={formatDate(form.expires_at)} onChange={(event) => updateForm("expires_at", event.target.value ? Math.floor(new Date(`${event.target.value}T00:00:00Z`).getTime() / 1000) : null)} /></label><label><span>Agent 上报间隔（秒）</span><input min="15" max="3600" type="number" value={form.report_interval} onChange={(event) => updateForm("report_interval", Number(event.target.value))} /></label><label><span>指标采样间隔（秒）</span><input min="1" max="60" type="number" value={form.collect_interval} onChange={(event) => updateForm("collect_interval", Number(event.target.value))} /></label></div>
        <div className="form-grid"><label><span>统计网卡（逗号分隔，留空自动）</span><input value={form.network_interface} onChange={(event) => updateForm("network_interface", event.target.value)} placeholder="eth0,ens3" /></label><label><span>下行流量当前值（GB）</span><input min="0" step="0.1" type="number" value={rxCurrentGb} onChange={(event) => setRxCurrentGb(event.target.value)} /></label><label><span>上行流量当前值（GB）</span><input min="0" step="0.1" type="number" value={txCurrentGb} onChange={(event) => setTxCurrentGb(event.target.value)} /></label><label><span>Agent 下载加速（可选）</span><input value={form.agent_mirror} onChange={(event) => updateForm("agent_mirror", event.target.value.trim())} placeholder="https://ghproxy.net" /></label></div>
        <div className="settings-toggles editor-toggles"><Toggle label="自动续费" checked={form.auto_renewal} onChange={(value) => updateForm("auto_renewal", value)} /><Toggle label="Agent 自动更新" checked={form.auto_update} onChange={(value) => updateForm("auto_update", value)} /><Toggle label="隐藏节点" checked={form.hidden} onChange={(value) => updateForm("hidden", value)} /><Toggle label="关闭离线告警" checked={form.offline_notify_disabled} onChange={(value) => updateForm("offline_notify_disabled", value)} /></div>
        <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => setEditing(null)}>取消</button><button className="primary-btn" disabled={busy}><Save size={16} />保存节点</button></div>
      </form></div> : null}

      {installCommand ? <div className="submodal-backdrop" role="presentation" onMouseDown={installDialog.onBackdropMouseDown}><section ref={installDialog.dialogRef} className="install-modal glass-panel" role="dialog" aria-modal="true" aria-labelledby="install-dialog-title" tabIndex={-1}><header><div><span className="eyebrow">Agent 部署</span><h3 id="install-dialog-title">安装命令</h3></div><div className="segmented install-platform" aria-label="Agent 平台"><button type="button" className={installPlatform === "linux" ? "active" : ""} onClick={() => setInstallPlatform("linux")}>Linux</button><button type="button" className={installPlatform === "windows" ? "active" : ""} onClick={() => setInstallPlatform("windows")}>Windows</button><button type="button" className={installPlatform === "macos" ? "active" : ""} onClick={() => setInstallPlatform("macos")}>macOS ARM</button><button type="button" className={installPlatform === "freebsd" ? "active" : ""} onClick={() => setInstallPlatform("freebsd")}>FreeBSD</button></div></header><div className="install-list"><pre>{installCommand}</pre></div><div className="form-actions"><button className="secondary-btn" type="button" onClick={() => setInstall(null)}>关闭</button><button className="primary-btn" type="button" onClick={() => void copyInstallCommand()}><Copy size={16} />复制</button></div></section></div> : null}
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

function DataStat({ label, value }: { label: string; value: number | string }) {
  return <div className="data-stat"><span>{label}</span><strong>{value}</strong></div>;
}

function UsageRow({ label, usage }: { label: string; usage: CloudflareUsage["today"] }) {
  return <tr><th scope="row">{label}</th><td>{usage.rows_read.toLocaleString()}</td><td>{usage.rows_written.toLocaleString()}</td><td>{usage.workers_requests.toLocaleString()}</td><td>{usage.durable_objects_requests.toLocaleString()}</td><td>{usage.durable_objects_duration.toLocaleString(undefined, { maximumFractionDigits: 2 })}</td></tr>;
}

function UsageDoBreakdown({ label, usage }: { label: string; usage: CloudflareUsage["today"] }) {
  return <div><strong>{label}</strong><span>HTTP {usage.durable_objects_http_requests.toLocaleString()}</span><span>休眠唤醒 {usage.durable_objects_hibernation_wakeups.toLocaleString()}</span><span>WS 入站 {usage.durable_objects_inbound_websocket_messages.toLocaleString()}</span><span>WS 出站 {usage.durable_objects_outbound_websocket_messages.toLocaleString()}</span><small>每 {usage.durable_objects_request_billing_ratio} 条入站消息折算 1 次请求</small></div>;
}
