import {
  ArrowLeft,
  CircleAlert,
  CircleCheck,
  LogOut,
  Moon,
  Save,
  Sun,
} from "lucide-react";
import { ChangeEvent, DragEvent, FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ADMIN_UNAUTHORIZED_EVENT, api, ApiError } from "../api";
import { ui } from "../locale";
import { adminTabFromPath, adminTabPaths, canonicalAdminPath, type AdminTab } from "../adminRoutes";
import { formatBytes, formatByteSize, parseByteSize } from "../format";
import { derivePassword } from "../password";
import { hasActiveRemoteTasks, isRemoteTaskActive, REMOTE_TASK_POLL_INTERVAL_MS, REMOTE_TASK_POLL_TIMEOUT_MS } from "../refresh";
import { ASSET_CURRENCIES, type AdminServer, type Config, type DatabaseMigrationResult, type DatabaseStats, type ExchangeRates, type LoginSession, type RemoteTask, type ServerInput, type Settings, type Theme, type ThemeSettingsSchema, type ThemeSettingValue, type TotpSetup, type TotpStatus } from "../types";
import { LoginForm } from "./LoginForm";
import { useDialog } from "./useDialog";
import { SiteLogo } from "./SiteLogo";
import { LatencyManager } from "./LatencyManager";
import { useVerification } from "./useVerification";
import { AboutTab } from "./admin/AboutTab";
import { RemoteTab } from "./admin/RemoteTab";
import { ServersTab } from "./admin/ServersTab";
import { InstallDialog } from "./admin/InstallDialog";
import { SettingsTabs } from "./admin/SettingsTabs";
import { ThemesTab } from "./admin/ThemesTab";
import { Toggle } from "./admin/controls";
import {
  adminNavigation,
  adminPages,
  billingCycles,
  copyText,
  describeLoginDevice,
  emptyServer,
  formatDate,
  settingPatch,
  toInput,
  waitForDatabaseSwitch,
  type AgentInstallInfo,
} from "./admin/shared";

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
  const [busy, setBusy] = useState(false);
  const locale = config.locale;
  const pages = adminPages(locale);
  const navigation = adminNavigation(locale);
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
  const databaseRestoreInputRef = useRef<HTMLInputElement>(null);
  const remoteTasksRef = useRef<RemoteTask[]>([]);
  const remoteTasksRequestRef = useRef(0);
  const remoteTasksActive = useMemo(() => hasActiveRemoteTasks(remoteTasks), [remoteTasks]);
  const remotePayloadReady = Boolean(remoteCommand.trim());
  const verificationDialog = useVerification(authenticated, locale);

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
      setError(reason instanceof Error ? reason.message : ui(locale, "加载失败", "Failed to load"));
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

  // These loaders are ordinary functions recreated on every render. Route the
  // tab effect through a ref so it always calls the latest version while its
  // dependencies stay [authenticated, tab]; listing them directly would either
  // be a stale-closure bug or refetch the active tab on every render.
  const tabLoaders = useRef({
    themeSettings: () => {},
    database: () => {},
    loginSessions: () => {},
  });
  useEffect(() => {
    tabLoaders.current = {
      themeSettings: loadThemeSettings,
      database: loadDatabase,
      loginSessions: loadLoginSessions,
    };
  });

  useEffect(() => {
    if (!authenticated) return;
    if (tab === "themeSettings") void tabLoaders.current.themeSettings();
    // `database` is read purely as a one-shot guard; re-running on every
    // database update would needlessly refetch the other tabs' data.
    if (tab === "data" && !database) void tabLoaders.current.database();
    if (tab === "security") void tabLoaders.current.loginSessions();
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

  async function handleLoginSubmit(loginUsername: string, loginPassword: string, turnstileToken: string, totpCode: string) {
    const passwordDerived = await derivePassword(loginPassword, config.password_client_salt, locale);
    await api.login(loginUsername.trim(), passwordDerived, turnstileToken, totpCode);
    setAuthenticated(true);
    await load();
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
      setError(reason instanceof Error ? reason.message : ui(locale, "保存节点失败", "Failed to save the server"));
    } finally { setBusy(false); }
  }

  async function removeSelected() {
    if (!selectedIds.length || !window.confirm(ui(locale, `确认删除选中的 ${selectedIds.length} 个节点及其全部历史数据？`, `Delete the selected ${selectedIds.length} server(s) and all their history?`))) return;
    setBusy(true);
    try {
      await api.deleteServers(selectedIds);
      setSelectedIds([]);
      await load();
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "批量删除失败", "Batch delete failed")); }
    finally { setBusy(false); }
  }

  async function remove(server: AdminServer) {
    if (!window.confirm(ui(locale, `确认删除“${server.name}”及其全部历史数据？`, `Delete "${server.name}" and all its history?`))) return;
    try { await api.deleteServer(server.id); await load(); onChanged(); }
    catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "删除失败", "Delete failed")); }
  }

  async function showInstallCommand(server: AdminServer) {
    setBusy(true);
    setError("");
    try {
      const { agent_token } = await api.createAgentInstallToken(server.id);
      setInstall({ agent_token, agent_mirror: server.agent_mirror });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(locale, "生成 Agent 安装命令失败", "Failed to generate the Agent install command"));
    } finally {
      setBusy(false);
    }
  }

  async function copyServerIp(ip: string) {
    try {
      await copyText(ip);
      setNotice(ui(locale, "IP 已复制", "IP copied"));
    } catch {
      setError(ui(locale, "复制失败，请手动选择复制", "Copy failed; please copy it manually"));
    }
  }

  async function move(index: number, offset: number) {
    const target = index + offset;
    if (target < 0 || target >= servers.length) return;
    const next = [...servers];
    [next[index], next[target]] = [next[target], next[index]];
    setServers(next);
    try { await api.reorderServers(next.map((server) => server.id)); onChanged(); }
    catch (reason) { setServers(servers); setError(reason instanceof Error ? reason.message : ui(locale, "排序失败", "Failed to reorder")); }
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
    catch (reason) { setServers(previous); setError(reason instanceof Error ? reason.message : ui(locale, "排序失败", "Failed to reorder")); }
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
        if (payload.new_password !== newPasswordConfirmation) throw new Error(ui(locale, "两次输入的新密码不一致", "The two new passwords do not match"));
        payload.new_password_derived = await derivePassword(payload.new_password, config.password_client_salt, locale);
        delete payload.new_password;
      } else {
        delete payload.new_password;
        delete payload.new_password_derived;
      }
      if (tab === "security") {
        const proof = await sensitiveProof(ui(locale, "保存设置", "Save settings"));
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
      setNotice(ui(locale, "设置已保存", "Settings saved"));
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "保存设置失败", "Failed to save settings")); }
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
    catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "读取数据库统计失败", "Failed to load database stats")); }
    finally { setBusy(false); }
  }

  async function sensitiveProof(action: string) {
    if (!twoFactorStatus) throw new Error(ui(locale, "正在读取两步验证状态，请稍候", "Reading two-factor status, please wait"));
    const entered = await verificationDialog.ask(action, twoFactorStatus.enabled);
    if (entered === null) return null;
    if (twoFactorStatus.enabled) return { totpCode: entered };
    return { passwordDerived: await derivePassword(entered, config.password_client_salt, locale) };
  }

  async function reclaimDatabase() {
    if (!window.confirm(ui(locale, "回收空间期间数据库会短暂不可用，确认继续？", "The database will be briefly unavailable while reclaiming space. Continue?"))) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const result = await api.reclaimDatabase();
      setDatabase(result.database);
      setNotice(result.reclaimed_bytes > 0 ? ui(locale, `已回收 ${formatBytes(result.reclaimed_bytes)}`, `Reclaimed ${formatBytes(result.reclaimed_bytes)}`) : ui(locale, "数据库已整理，当前没有可释放空间", "Database optimized; nothing to reclaim right now"));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "回收数据库空间失败", "Failed to reclaim database space")); }
    finally { setBusy(false); }
  }

  async function migrateDatabase() {
    const databaseUrl = databaseMigrationUrl.trim();
    if (!databaseUrl) return;
    const source = database?.kind === "postgresql" ? "PostgreSQL" : "SQLite";
    const target = database?.kind === "postgresql" ? "SQLite" : "PostgreSQL";
    if (!window.confirm(ui(locale, `将 ${source} 迁移到 ${target}？目标库中已有的 NodeFlare 数据会被覆盖，完成后业务写入将暂停，直至重启服务。`, `Migrate from ${source} to ${target}? Existing NodeFlare data in the target database will be overwritten, and writes will pause until the service restarts.`))) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof(ui(locale, "迁移数据库", "Migrate database"));
      if (!proof) return;
      const result = await api.migrateDatabase(databaseUrl, proof);
      setDatabaseMigrationUrl("");
      setDatabaseMigrationResult(result);
      setDatabase((current) => current ? { ...current, restart_required: result.restart_required } : current);
      const targetName = result.target_kind === "postgresql" ? "PostgreSQL" : "SQLite";
      setNotice(ui(locale, `数据库已迁移到 ${targetName}，共 ${result.migrated_rows.toLocaleString()} 行（${formatBytes(result.size_bytes)}），业务写入已暂停，请重启服务`, `Migrated to ${targetName}: ${result.migrated_rows.toLocaleString()} rows (${formatBytes(result.size_bytes)}). Writes are paused; restart the service`));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "数据库迁移失败", "Database migration failed")); }
    finally { setBusy(false); }
  }

  async function restartAfterDatabaseMigration() {
    if (!database?.restart_required || !window.confirm(ui(locale, "立即重启 NodeFlare 并切换到新数据库？管理界面和 Agent 会短暂断开。", "Restart NodeFlare now and switch to the new database? The admin UI and agents will disconnect briefly."))) return;
    const targetKind = databaseMigrationResult?.target_kind === "postgresql"
      ? "postgresql"
      : databaseMigrationResult?.target_kind === "sqlite"
        ? "sqlite"
        : database.kind === "postgresql" ? "sqlite" : "postgresql";
    setBusy(true); setRestarting(true); setError(""); setNotice(ui(locale, "NodeFlare 正在重启", "NodeFlare is restarting"));
    try {
      await api.restartAfterDatabaseMigration();
      await waitForDatabaseSwitch(targetKind, locale);
      window.location.replace("/admin/login");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(locale, "重启 NodeFlare 失败", "Failed to restart NodeFlare"));
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
      setNotice(ui(locale, `汇率已更新 · ${rates.source} · ${rates.date}`, `Exchange rates updated · ${rates.source} · ${rates.date}`));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "汇率更新失败", "Failed to refresh exchange rates")); }
    finally { setBusy(false); }
  }

  async function exportDatabaseBackup() {
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof(ui(locale, "导出备份", "Export backup"));
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
      setNotice(ui(locale, "数据库备份已导出", "Database backup exported"));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "导出数据库备份失败", "Failed to export the database backup")); }
    finally { setBusy(false); }
  }

  async function restoreDatabaseBackup(file: File) {
    if (file.size > 512 * 1024 * 1024) {
      setError(ui(locale, "数据库备份 ZIP 不能超过 512 MiB", "Database backup ZIP must not exceed 512 MiB"));
      return;
    }
    if (!window.confirm(ui(locale, "恢复将覆盖当前数据库，并使现有登录会话失效。确认继续？", "Restoring overwrites the current database and invalidates existing sessions. Continue?"))) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof(ui(locale, "恢复备份", "Restore backup"));
      if (!proof) return;
      const result = await api.restoreDatabaseBackup(file, proof);
      window.alert(ui(locale, `数据库已恢复 ${result.restored_rows.toLocaleString()} 行，请使用备份中的管理员账号重新登录。`, `Restored ${result.restored_rows.toLocaleString()} rows. Sign in again with the admin account from the backup.`));
      window.location.reload();
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "恢复数据库备份失败", "Failed to restore the database backup")); }
    finally { setBusy(false); }
  }

  async function loadThemeSettings() {
    setBusy(true); setError("");
    try { setThemeSettingsSchema(await api.themeSettings()); }
    catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "读取主题设置失败", "Failed to load theme settings")); }
    finally { setBusy(false); }
  }

  async function addTheme(
    input: { name: string; description: string; url: string },
    file: File | null,
  ): Promise<boolean> {
    setBusy(true); setError(""); setNotice("");
    try {
      if (file) {
        await api.uploadTheme({ name: input.name, description: input.description }, file);
      } else {
        await api.addTheme({ name: input.name, description: input.description, url: input.url });
      }
      await load();
      setNotice(file ? ui(locale, "主题 ZIP 已安装", "Theme ZIP installed") : ui(locale, "最新 Release 主题已安装", "Latest release theme installed"));
      return true;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(locale, "添加主题失败", "Failed to add the theme"));
      return false;
    } finally { setBusy(false); }
  }

  async function activateTheme(theme: Theme) {
    if (theme.active) return;
    setBusy(true); setError(""); setNotice("");
    try {
      await api.activateTheme(theme.id);
      await load();
      setNotice(ui(locale, `已启用主题：${theme.name}`, `Theme activated: ${theme.name}`));
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "启用主题失败", "Failed to activate the theme")); }
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
      setError(reason instanceof Error ? reason.message : ui(locale, "创建主题预览失败", "Failed to create the theme preview"));
    } finally { setBusy(false); }
  }

  async function removeTheme(theme: Theme) {
    if (theme.builtin || !window.confirm(ui(locale, `确认删除主题“${theme.name}”？`, `Delete the theme "${theme.name}"?`))) return;
    setBusy(true); setError(""); setNotice("");
    try {
      await api.deleteTheme(theme.id);
      await load();
      setNotice(ui(locale, "主题已删除", "Theme deleted"));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "删除主题失败", "Failed to delete the theme")); }
    finally { setBusy(false); }
  }

  async function loadLoginSessions() {
    try {
      const result = await api.loginSessions();
      setLoginSessions(result.sessions);
      setLoginSessionsLoaded(true);
    } catch (reason) {
      if (!(reason instanceof ApiError && reason.status === 401)) {
        setError(reason instanceof Error ? reason.message : ui(locale, "读取登录设备失败", "Failed to load signed-in devices"));
      }
    }
  }

  async function revokeLoginSession(session: LoginSession) {
    if (session.current || !window.confirm(ui(locale, `确认让“${describeLoginDevice(session.user_agent, locale)}”下线？`, `Sign out "${describeLoginDevice(session.user_agent, locale)}"?`))) return;
    setRevokingSessionId(session.id); setError(""); setNotice("");
    try {
      await api.revokeLoginSession(session.id);
      setLoginSessions((current) => current.filter((item) => item.id !== session.id));
      setNotice(ui(locale, "设备已踢下线", "Device signed out"));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(locale, "设备下线失败", "Failed to sign out the device"));
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
      const proof = await sensitiveProof(ui(locale, "生成两步验证密钥", "Generate two-factor secret"));
      if (!proof) return;
      const setup = await api.setupTwoFactor(proof);
      setTwoFactorSetup(setup);
      setTwoFactorStatus({ enabled: false, has_secret: true });
      setTwoFactorSecretCopied(false);
      setNotice(ui(locale, "两步验证密钥已生成，请先加入验证器再启用", "Two-factor secret generated; add it to your authenticator before enabling"));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "生成两步验证密钥失败", "Failed to generate the two-factor secret")); }
    finally { setBusy(false); }
  }

  async function enableTwoFactor() {
    setBusy(true); setError(""); setNotice("");
    try {
      const code = await verificationDialog.ask(ui(locale, "启用两步验证", "Enable two-factor authentication"), true);
      if (code === null) return;
      await api.enableTwoFactor(code);
      setTwoFactorStatus({ enabled: true, has_secret: true });
      setTwoFactorSetup(null);
      setTwoFactorSecretCopied(false);
      setNotice(ui(locale, "两步验证已启用", "Two-factor authentication enabled"));
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "启用两步验证失败", "Failed to enable two-factor authentication")); }
    finally { setBusy(false); }
  }

  async function disableTwoFactor() {
    setBusy(true); setError(""); setNotice("");
    try {
      const code = await verificationDialog.ask(ui(locale, "禁用两步验证", "Disable two-factor authentication"), true);
      if (code === null) return;
      await api.disableTwoFactor(code);
      setTwoFactorStatus({ enabled: false, has_secret: true });
      setNotice(ui(locale, "两步验证已禁用", "Two-factor authentication disabled"));
      onChanged();
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "禁用两步验证失败", "Failed to disable two-factor authentication")); }
    finally { setBusy(false); }
  }

  async function copyTwoFactorSecret() {
    if (!twoFactorSetup) return;
    try {
      await copyText(twoFactorSetup.secret);
      setTwoFactorSecretCopied(true);
    } catch {
      setError(ui(locale, "复制失败，请手动选择复制", "Copy failed; please copy it manually"));
    }
  }

  const serverDialog = useDialog<HTMLFormElement>(editing !== null, () => setEditing(null));

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
    if (failed && !quiet) setError(ui(locale, "部分执行结果刷新失败", "Some results failed to refresh"));
  }, []);

  const createRemoteTask = async (event: FormEvent) => {
    event.preventDefault();
    if (!remoteSelectedIds.length) {
      setError(ui(locale, "请选择至少一台服务器", "Select at least one server"));
      return;
    }
    if (!remoteCommand.trim()) {
      setError(ui(locale, "请输入命令", "Enter a command"));
      return;
    }
    if (twoFactorStatus === null) {
      setError(ui(locale, "正在读取两步验证状态，请稍候", "Reading two-factor status, please wait"));
      return;
    }
    if (!twoFactorStatus.enabled) {
      setError(ui(locale, "请先在登录与安全中启用 TOTP 两步验证", "Enable TOTP two-factor authentication under Sign-in & security first"));
      return;
    }

    setBusy(true);
    setError("");
    try {
      const code = await verificationDialog.ask(ui(locale, "确认执行远程命令", "Confirm remote command execution"), true);
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
      setNotice(ui(locale, "已提交命令，请查看各节点的执行结果", "Command submitted; check each node for results"));
      await refreshRemoteTasks(true, true);
    } catch (err) {
      setError(err instanceof Error && err.name === "TimeoutError"
        ? ui(locale, "下发请求超时，命令可能已提交，请先检查节点，避免重复执行", "Dispatch timed out; the command may have been submitted. Check the nodes before retrying")
        : err instanceof Error ? err.message : ui(locale, "下发命令失败", "Failed to dispatch the command"));
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
        <button className="admin-login-theme" type="button" onClick={onToggleTheme} title={dark ? ui(locale, "切换浅色主题", "Switch to light theme") : ui(locale, "切换深色主题", "Switch to dark theme")}>{dark ? <Sun size={15} /> : <Moon size={15} />}</button>
        <LoginForm
          config={config}
          dark={dark}
          error={error}
          setError={setError}
          onSubmit={handleLoginSubmit}
          className="login-form glass-panel"
          turnstileClassName="login-turnstile"
          errorClassName="login-error"
          submitClassName="primary-btn login-submit"
        >
          <SiteLogo src={siteLogoUrl} alt="" width="48" height="48" />
          <div className="login-copy"><h1>{ui(locale, "管理员登录", "Admin sign in")}</h1><p>{config.site_name}</p></div>
        </LoginForm>
      </div> : <section className="admin-shell" aria-label={ui(locale, "管理面板", "Admin panel")}>
        <header className="admin-topbar">
          <div className="admin-brand">
            <SiteLogo src={siteLogoUrl} alt="" width="36" height="36" />
            <strong>{config.site_name}</strong>
          </div>
          <div className="admin-topbar-actions">
            <button type="button" onClick={onToggleTheme} title={dark ? ui(locale, "切换浅色主题", "Switch to light theme") : ui(locale, "切换深色主题", "Switch to dark theme")} aria-label={dark ? ui(locale, "切换浅色主题", "Switch to light theme") : ui(locale, "切换深色主题", "Switch to dark theme")}>{dark ? <Sun size={15} /> : <Moon size={15} />}</button>
            <a className="admin-home-link" href="/" target="_blank" rel="noopener noreferrer" title={ui(locale, "主页", "Home")} aria-label={ui(locale, "主页", "Home")}><ArrowLeft size={15} /></a>
            <button type="button" onClick={() => void logout()} title={ui(locale, "退出", "Sign out")} aria-label={ui(locale, "退出", "Sign out")}><LogOut size={15} /></button>
          </div>
        </header>

          <div className="admin-body">
            <aside className="admin-sidebar">
              <nav className="admin-tabs" aria-label={ui(locale, "管理导航", "Admin navigation")}>
                {navigation.map((item) => {
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
              <header className="admin-content-header"><h1>{pages[tab].title}</h1><p>{pages[tab].description}</p></header>
              {tab === "servers" ? (
                <ServersTab
                  locale={locale}
                  servers={servers}
                  busy={busy}
                  selectedIds={selectedIds}
                  allSelected={allSelected}
                  draggingId={draggingId}
                  onCopyIp={(ip) => void copyServerIp(ip)}
                  onAdd={() => openEditor()}
                  onToggleSelected={toggleSelected}
                  onToggleAll={() => setSelectedIds(allSelected ? [] : servers.map((server) => server.id))}
                  onRemoveSelected={() => void removeSelected()}
                  onMove={(index, offset) => void move(index, offset)}
                  onStartDrag={startDrag}
                  onEndDrag={() => setDraggingId("")}
                  onDrop={(event, id) => void dropServer(event, id)}
                  onInstallCommand={(server) => void showInstallCommand(server)}
                  onEdit={(server) => openEditor(server)}
                  onRemove={(server) => void remove(server)}
                />
              ) : tab === "latency" ? (
                <LatencyManager locale={locale} servers={servers} onError={setError} onNotice={setNotice} />
              ) : tab === "themes" && settings ? (
                <ThemesTab
                  locale={locale}
                  themes={themes}
                  busy={busy}
                  onActivate={(theme) => void activateTheme(theme)}
                  onPreview={(theme) => void previewTheme(theme)}
                  onRemove={(theme) => void removeTheme(theme)}
                  onAdd={addTheme}
                  onError={setError}
                />
              ) : settings && (tab === "appearance" || tab === "themeSettings" || tab === "alerts" || tab === "security" || tab === "data") ? (
                <SettingsTabs
                  tab={tab}
                  locale={locale}
                  settings={settings}
                  servers={servers}
                  busy={busy}
                  saveSite={saveSite}
                  migrateDatabase={migrateDatabase}
                  updateSettings={updateSettings}
                  themeSettingsSchema={themeSettingsSchema}
                  updateThemeOption={updateThemeOption}
                  setError={setError}
                  setNotice={setNotice}
                  newPasswordConfirmation={newPasswordConfirmation}
                  setNewPasswordConfirmation={setNewPasswordConfirmation}
                  twoFactorStatus={twoFactorStatus}
                  twoFactorSetup={twoFactorSetup}
                  twoFactorSecretCopied={twoFactorSecretCopied}
                  copyTwoFactorSecret={copyTwoFactorSecret}
                  setupTwoFactor={setupTwoFactor}
                  enableTwoFactor={enableTwoFactor}
                  disableTwoFactor={disableTwoFactor}
                  loginSessions={loginSessions}
                  loginSessionsLoaded={loginSessionsLoaded}
                  loadLoginSessions={loadLoginSessions}
                  revokeLoginSession={revokeLoginSession}
                  revokingSessionId={revokingSessionId}
                  database={database}
                  databaseRestoreInputRef={databaseRestoreInputRef}
                  databaseMigrationUrl={databaseMigrationUrl}
                  setDatabaseMigrationUrl={setDatabaseMigrationUrl}
                  databaseMigrationResult={databaseMigrationResult}
                  exportDatabaseBackup={exportDatabaseBackup}
                  restoreDatabaseBackup={restoreDatabaseBackup}
                  reclaimDatabase={reclaimDatabase}
                  restarting={restarting}
                  restartAfterDatabaseMigration={restartAfterDatabaseMigration}
                  exchangeRates={exchangeRates}
                  exchangeRatesExpanded={exchangeRatesExpanded}
                  setExchangeRatesExpanded={setExchangeRatesExpanded}
                  refreshExchangeRates={refreshExchangeRates}
                />
              ) : tab === "remote" ? (
                <RemoteTab
                  locale={locale}
                  servers={servers}
                  busy={busy}
                  selectTab={selectTab}
                  createRemoteTask={createRemoteTask}
                  remoteCommand={remoteCommand}
                  setRemoteCommand={setRemoteCommand}
                  remoteSelectedIds={remoteSelectedIds}
                  setRemoteSelectedIds={setRemoteSelectedIds}
                  remoteAllSelected={remoteAllSelected}
                  remoteQuery={remoteQuery}
                  setRemoteQuery={setRemoteQuery}
                  remoteVisibleServers={remoteVisibleServers}
                  toggleRemoteServer={toggleRemoteServer}
                  remotePayloadReady={remotePayloadReady}
                  remoteTasks={remoteTasks}
                  remoteTasksActive={remoteTasksActive}
                  remotePollingUntil={remotePollingUntil}
                  setRemotePollingUntil={setRemotePollingUntil}
                  refreshRemoteTasks={refreshRemoteTasks}
                  remoteServerById={remoteServerById}
                  twoFactorStatus={twoFactorStatus}
                />
              ) : tab === "about" ? (
                <AboutTab locale={locale} />
              ) : null}
            </div>
          </div>
      </section>}

      {editing ? <div className="submodal-backdrop" role="presentation" onMouseDown={serverDialog.onBackdropMouseDown}><form ref={serverDialog.dialogRef} className="editor-modal glass-panel" role="dialog" aria-modal="true" aria-labelledby="server-editor-title" tabIndex={-1} onSubmit={saveServer}>
        <header><div><span className="eyebrow">{ui(locale, "节点配置", "Server configuration")}</span><h3 id="server-editor-title">{editing === "new" ? ui(locale, "添加节点", "Add server") : ui(locale, `编辑 · ${editing.name}`, `Edit · ${editing.name}`)}</h3></div></header>
        <div className="form-grid"><label><span>{ui(locale, "名称", "Name")}</span><input autoFocus required value={form.name} onChange={(event) => updateForm("name", event.target.value)} /></label><label><span>{ui(locale, "地区代码", "Region code")}</span><input maxLength={16} placeholder="CN / JP / DE" value={form.region} onChange={(event) => updateForm("region", event.target.value.toUpperCase())} /></label><label><span>{ui(locale, "分组", "Group")}</span><input value={form.group_name} onChange={(event) => updateForm("group_name", event.target.value)} /></label><label><span>{ui(locale, "标签", "Tags")}</span><input placeholder={ui(locale, "主力, 线路:BGP", "primary, line:BGP")} value={form.tags} onChange={(event) => updateForm("tags", event.target.value)} /></label></div>
        <div className="form-grid three"><label><span>{ui(locale, "流量限额（0 不限）", "Traffic limit (0 = unlimited)")}</span><input placeholder={ui(locale, "如 100 G，不带单位按 GB；0 不限", "e.g. 100 G; bare numbers mean GB; 0 = unlimited")} {...sizeInputProps(trafficLimitText, setTrafficLimitText, (bytes) => updateForm("traffic_limit", bytes), form.traffic_limit)} /></label><label><span>{ui(locale, "流量口径", "Traffic accounting")}</span><select value={form.traffic_limit_type} onChange={(event) => updateForm("traffic_limit_type", event.target.value as ServerInput["traffic_limit_type"])}><option value="sum">{ui(locale, "上下行合计", "Up + down")}</option><option value="max">{ui(locale, "取较大值", "Larger of the two")}</option><option value="min">{ui(locale, "取较小值", "Smaller of the two")}</option><option value="up">{ui(locale, "仅上行", "Upload only")}</option><option value="down">{ui(locale, "仅下行", "Download only")}</option></select></label><label><span>{ui(locale, "流量重置日", "Traffic reset day")}</span><input min="1" max="31" type="number" value={form.reset_day} onChange={(event) => updateForm("reset_day", Number(event.target.value))} /></label></div>
        <div className="form-grid three"><label><span>{ui(locale, "价格（0 免费）", "Price (0 = free)")}</span><input type="number" min="0" max="1000000000" step="any" inputMode="decimal" value={priceText} onChange={(event) => updatePriceText(event.target.value)} onBlur={() => setPriceText(String(form.price))} /></label><label><span>{ui(locale, "币种", "Currency")}</span><select value={form.currency} onChange={(event) => updateForm("currency", event.target.value)}>{ASSET_CURRENCIES.map((code) => <option key={code}>{code}</option>)}</select></label><label><span>{ui(locale, "计费周期", "Billing cycle")}</span><select value={String(form.billing_cycle)} onChange={(event) => updateForm("billing_cycle", Number(event.target.value))}>{billingCycles(locale).map((cycle) => <option key={cycle.days} value={cycle.days}>{cycle.label}</option>)}{billingCycles(locale).every((cycle) => cycle.days !== form.billing_cycle) ? <option value={form.billing_cycle}>{ui(locale, `${form.billing_cycle} 天`, `${form.billing_cycle} days`)}</option> : null}</select></label></div>
        <div className="form-grid three"><label><span>{ui(locale, "到期日期", "Expiry date")}</span><input type="date" value={formatDate(form.expires_at)} onChange={(event) => updateForm("expires_at", event.target.value ? Math.floor(new Date(`${event.target.value}T00:00:00Z`).getTime() / 1000) : null)} /></label><label><span>{ui(locale, "历史保存间隔（秒）", "History interval (s)")}</span><input min="15" max="3600" type="number" value={form.report_interval} onChange={(event) => updateForm("report_interval", Number(event.target.value))} /></label><label><span>{ui(locale, "实时上传间隔（秒，每秒采样）", "Live upload interval (s, sampled every second)")}</span><input min="3" max="60" type="number" value={form.collect_interval} onChange={(event) => updateForm("collect_interval", Number(event.target.value))} /></label></div>
        <div className="form-grid"><label><span>{ui(locale, "统计网卡（支持 * 通配和 ! 排除，留空自动）", "Network interface (* wildcard and ! exclusion; empty = auto)")}</span><input value={form.network_interface} onChange={(event) => updateForm("network_interface", event.target.value)} placeholder="eth*,!eth1" /></label><label><span>{ui(locale, "Agent 下载加速（可选）", "Agent download mirror (optional)")}</span><input value={form.agent_mirror} onChange={(event) => updateForm("agent_mirror", event.target.value.trim())} placeholder="https://ghproxy.net" /></label><label><span>{ui(locale, "上行流量当前值", "Current upload traffic")}</span><input placeholder={ui(locale, "如 500 G", "e.g. 500 G")} {...sizeInputProps(txCurrentText, setTxCurrentText, setTxCurrentBytes, txCurrentBytes)} /></label><label><span>{ui(locale, "下行流量当前值", "Current download traffic")}</span><input placeholder={ui(locale, "如 500 G", "e.g. 500 G")} {...sizeInputProps(rxCurrentText, setRxCurrentText, setRxCurrentBytes, rxCurrentBytes)} /></label></div>
        <div className="settings-toggles editor-toggles"><Toggle label={form.billing_cycle <= 0 ? ui(locale, "自动续费（一次性不适用）", "Auto-renew (N/A for one-time)") : ui(locale, "自动续费", "Auto-renew")} checked={form.auto_renewal} onChange={(value) => updateForm("auto_renewal", value)} /><Toggle label={ui(locale, "Agent 自动更新", "Agent auto-update")} checked={form.auto_update} onChange={(value) => updateForm("auto_update", value)} /><Toggle label={ui(locale, "隐藏节点", "Hide server")} checked={form.hidden} onChange={(value) => updateForm("hidden", value)} /><Toggle label={ui(locale, "关闭离线告警", "Disable offline alerts")} checked={form.offline_notify_disabled} onChange={(value) => updateForm("offline_notify_disabled", value)} /></div>
        <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => setEditing(null)}>{ui(locale, "取消", "Cancel")}</button><button className="primary-btn" disabled={busy}><Save size={15} />{ui(locale, "保存节点", "Save server")}</button></div>
      </form></div> : null}

      {install ? <InstallDialog locale={locale} install={install} onClose={() => setInstall(null)} onNotice={setNotice} onError={setError} /> : null}
      {verificationDialog.dialog}
    </div>
  );
}

