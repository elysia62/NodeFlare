import { AlertTriangle, ArrowRightLeft, Check, ChevronDown, ChevronUp, CircleCheck, Coins, Copy, Download, Eye, LogOut, MonitorSmartphone, Power, RotateCw, Save, ShieldCheck, SlidersHorizontal, Upload } from "lucide-react";
import type { Dispatch, FormEvent, RefObject, SetStateAction } from "react";
import type { AdminTab } from "../adminRoutes";
import { formatBytes } from "../format";
import { ui, type UiLocale } from "../locale";
import { ASSET_CURRENCIES, type AdminServer, type DatabaseMigrationResult, type DatabaseStats, type ExchangeRates, type LoginSession, type Settings, type ThemeSettingsSchema, type ThemeSettingValue, type TotpSetup, type TotpStatus } from "../types";
import { AlertRuleManager } from "../AlertRuleManager";
import { PasswordInput } from "../PasswordInput";
import { TelegramSettings } from "../TelegramSettings";
import { ThemeOption, Toggle } from "./controls";
import { describeLoginDevice, formatSessionTime } from "./shared";

export interface SettingsTabsProps {
  tab: AdminTab;
  locale: UiLocale | string | undefined;
  settings: Settings;
  servers: AdminServer[];
  busy: boolean;
  saveSite: (event: FormEvent) => void;
  migrateDatabase: () => void;
  updateSettings: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  themeSettingsSchema: ThemeSettingsSchema | null;
  updateThemeOption: (key: string, value: ThemeSettingValue) => void;
  setError: (message: string) => void;
  setNotice: (message: string) => void;
  newPasswordConfirmation: string;
  setNewPasswordConfirmation: (value: string) => void;
  twoFactorStatus: TotpStatus | null;
  twoFactorSetup: TotpSetup | null;
  twoFactorSecretCopied: boolean;
  copyTwoFactorSecret: () => void;
  setupTwoFactor: () => void;
  enableTwoFactor: () => void;
  disableTwoFactor: () => void;
  loginSessions: LoginSession[];
  loginSessionsLoaded: boolean;
  loadLoginSessions: () => void;
  revokeLoginSession: (session: LoginSession) => void;
  revokingSessionId: string;
  database: DatabaseStats | null;
  databaseRestoreInputRef: RefObject<HTMLInputElement | null>;
  databaseMigrationUrl: string;
  setDatabaseMigrationUrl: (value: string) => void;
  databaseMigrationResult: DatabaseMigrationResult | null;
  exportDatabaseBackup: () => void;
  restoreDatabaseBackup: (file: File) => void;
  reclaimDatabase: () => void;
  restarting: boolean;
  restartAfterDatabaseMigration: () => void;
  exchangeRates: ExchangeRates | null;
  exchangeRatesExpanded: boolean;
  setExchangeRatesExpanded: Dispatch<SetStateAction<boolean>>;
  refreshExchangeRates: () => void;
}

export function SettingsTabs({ tab, locale, settings, servers, busy, saveSite, migrateDatabase, updateSettings, themeSettingsSchema, updateThemeOption, setError, setNotice, newPasswordConfirmation, setNewPasswordConfirmation, twoFactorStatus, twoFactorSetup, twoFactorSecretCopied, copyTwoFactorSecret, setupTwoFactor, enableTwoFactor, disableTwoFactor, loginSessions, loginSessionsLoaded, loadLoginSessions, revokeLoginSession, revokingSessionId, database, databaseRestoreInputRef, databaseMigrationUrl, setDatabaseMigrationUrl, databaseMigrationResult, exportDatabaseBackup, restoreDatabaseBackup, reclaimDatabase, restarting, restartAfterDatabaseMigration, exchangeRates, exchangeRatesExpanded, setExchangeRatesExpanded, refreshExchangeRates }: SettingsTabsProps) {
  return (
    <form className="settings-form" onSubmit={tab === "data" ? (event) => { event.preventDefault(); void migrateDatabase(); } : saveSite}>
      {tab === "appearance" ? <>
        <div className="section-title"><Eye size={15} />{ui(locale, "外观与展示", "Appearance")}</div>
        <div className="form-grid"><label><span>{ui(locale, "站点名称", "Site name")}</span><input required value={settings.site_name} onChange={(event) => updateSettings("site_name", event.target.value)} /></label><label><span>{ui(locale, "站点描述", "Site description")}</span><input value={settings.site_description} onChange={(event) => updateSettings("site_description", event.target.value)} /></label></div>
        <label><span>{ui(locale, "站点公告", "Site announcement")}</span><textarea rows={3} maxLength={1000} value={settings.site_announcement} onChange={(event) => updateSettings("site_announcement", event.target.value)} /></label>
        <div className="form-grid"><label><span>{ui(locale, "界面语言", "Language")}</span><select value={settings.locale} onChange={(event) => updateSettings("locale", event.target.value as Settings["locale"])}><option value="zh-CN">简体中文</option><option value="en">English</option></select></label><label><span>{ui(locale, "站点 Logo / 浏览器图标地址", "Site logo / favicon URL")}</span><input type="url" maxLength={1000} value={settings.logo_url} onChange={(event) => updateSettings("logo_url", event.target.value)} placeholder="https://example.com/logo.svg" /></label></div>
        <div className="form-grid"><label><span>{ui(locale, "离线判定（秒）", "Offline threshold (s)")}</span><input type="number" min="30" max="3600" value={settings.offline_threshold_seconds} onChange={(event) => updateSettings("offline_threshold_seconds", Number(event.target.value))} /></label><label><span>{ui(locale, "历史保留（天）", "History retention (days)")}</span><input type="number" min="1" max="30" value={settings.history_retention_days} onChange={(event) => updateSettings("history_retention_days", Number(event.target.value))} /></label></div>
      </> : null}

      {tab === "themeSettings" ? <>
        <div className="section-title"><SlidersHorizontal size={15} />{ui(locale, "通用主题设置", "General theme settings")}</div>
        <div className="form-grid"><label><span>{ui(locale, "默认主题", "Default theme")}</span><select value={settings.default_theme} onChange={(event) => updateSettings("default_theme", event.target.value as Settings["default_theme"])}><option value="system">{ui(locale, "跟随系统", "System")}</option><option value="light">{ui(locale, "浅色", "Light")}</option><option value="dark">{ui(locale, "深色", "Dark")}</option></select></label><label><span>{ui(locale, "背景图地址", "Background image URL")}</span><input type="text" maxLength={1000} value={settings.background_url} onChange={(event) => updateSettings("background_url", event.target.value)} placeholder="https://example.com/light.webp | https://example.com/dark.webp" /></label></div>
        <p className="settings-hint">{ui(locale, "仅支持 HTTPS 地址；浅色和深色背景可用 | 分隔。", "HTTPS only; separate light and dark backgrounds with |.")}</p>
        <div className="section-subtitle">{ui(locale, "当前主题选项", "Active theme options")}</div><div className="theme-option-grid"><Toggle label={ui(locale, "公开仪表盘", "Public dashboard")} checked={settings.public_dashboard} onChange={(value) => updateSettings("public_dashboard", value)} />{themeSettingsSchema?.settings.map((field) => <ThemeOption key={field.key} field={field} value={settings.theme_options[field.key] ?? field.default} onChange={(value) => updateThemeOption(field.key, value)} />)}</div>
        <div className="section-subtitle">{ui(locale, "公开界面元素", "Public UI elements")}</div>
        <div className="settings-toggles"><Toggle label={ui(locale, "显示搜索", "Show search")} checked={settings.show_search} onChange={(value) => updateSettings("show_search", value)} /><Toggle label={ui(locale, "显示分组", "Show groups")} checked={settings.show_groups} onChange={(value) => updateSettings("show_groups", value)} /><Toggle label={ui(locale, "总览统计", "Overview stats")} checked={settings.show_stats} onChange={(value) => updateSettings("show_stats", value)} /><Toggle label={ui(locale, "资产统计", "Asset stats")} checked={settings.show_assets} onChange={(value) => updateSettings("show_assets", value)} /><Toggle label={ui(locale, "累计流量", "Total traffic")} checked={settings.show_traffic} onChange={(value) => updateSettings("show_traffic", value)} /><Toggle label={ui(locale, "实时网速", "Live speed")} checked={settings.show_speed} onChange={(value) => updateSettings("show_speed", value)} /><Toggle label={ui(locale, "价格信息", "Price info")} checked={settings.show_price} onChange={(value) => updateSettings("show_price", value)} /><Toggle label={ui(locale, "到期信息", "Expiry info")} checked={settings.show_expiry} onChange={(value) => updateSettings("show_expiry", value)} /><Toggle label={ui(locale, "延迟与丢包", "Latency & loss")} checked={settings.show_latency} onChange={(value) => updateSettings("show_latency", value)} /><Toggle label={ui(locale, "在线时长", "Uptime")} checked={settings.show_uptime} onChange={(value) => updateSettings("show_uptime", value)} /></div>
      </> : null}

      {tab === "alerts" ? <>
        <div className="section-title"><AlertTriangle size={15} />{ui(locale, "通知与告警", "Notifications & alerts")}</div>
        <Toggle label={ui(locale, "启用通知与告警", "Enable notifications & alerts")} checked={settings.notification_enabled} onChange={(value) => updateSettings("notification_enabled", value)} />
        <div className="form-grid three"><label><span>{ui(locale, "离线告警延迟（分钟）", "Offline alert delay (min)")}</span><input type="number" min="2" max="1440" value={settings.offline_alert_minutes} onChange={(event) => updateSettings("offline_alert_minutes", Number(event.target.value))} /></label><label><span>{ui(locale, "到期提醒（天）", "Expiry reminder (days)")}</span><input type="number" min="0" max="365" value={settings.expiry_alert_days} onChange={(event) => updateSettings("expiry_alert_days", Number(event.target.value))} /></label><label><span>{ui(locale, "流量提醒起始阈值（%）", "Traffic alert threshold (%)")}</span><input type="number" min="50" max="100" value={settings.traffic_alert_percentage} onChange={(event) => updateSettings("traffic_alert_percentage", Number(event.target.value))} /></label></div>
        <p className="settings-hint">{ui(locale, "流量达到起始阈值后，每增加 5 个百分点生成一次事件，最多提醒到 100%。", "After the threshold is reached, an event is created for every additional 5 percentage points, up to 100%.")}</p>
        <TelegramSettings locale={locale} onError={setError} onNotice={setNotice} />
        <AlertRuleManager locale={locale} servers={servers} onError={setError} onNotice={setNotice} />
      </> : null}

      {tab === "security" ? <>
        <div className="section-title"><ShieldCheck size={15} />{ui(locale, "账号与 Cloudflare 防护", "Account & Cloudflare protection")}</div>
        <div className="form-grid three"><label><span>{ui(locale, "管理员用户名", "Admin username")}</span><input autoComplete="username" value={settings.admin_username} onChange={(event) => updateSettings("admin_username", event.target.value)} /></label><label><span>{ui(locale, "新密码（留空不修改）", "New password (leave empty to keep)")}</span><PasswordInput locale={locale} autoComplete="new-password" minLength={8} maxLength={128} value={settings.new_password || ""} onChange={(event) => { updateSettings("new_password", event.target.value); setNewPasswordConfirmation(""); }} placeholder={ui(locale, "至少 8 个字符", "At least 8 characters")} /></label><label><span>{ui(locale, "确认新密码", "Confirm new password")}</span><PasswordInput locale={locale} autoComplete="new-password" minLength={8} maxLength={128} value={newPasswordConfirmation} onChange={(event) => setNewPasswordConfirmation(event.target.value)} placeholder={ui(locale, "再次输入新密码", "Re-enter the new password")} /></label></div>
        <div className="two-factor-panel">
          <div className="two-factor-head"><div><div className="section-subtitle">{ui(locale, "TOTP 两步验证", "TOTP two-factor authentication")}</div><p className="settings-hint">{ui(locale, "启用后管理员登录和每次远程执行都必须提交验证器生成的 6 位动态码。", "Once enabled, admin sign-in and every remote execution require the 6-digit code from your authenticator.")}</p></div><span className={`two-factor-status ${twoFactorStatus?.enabled ? "enabled" : ""}`}>{twoFactorStatus?.enabled ? ui(locale, "已启用", "Enabled") : twoFactorStatus ? ui(locale, "未启用", "Not enabled") : ui(locale, "读取中", "Loading")}</span></div>
          {twoFactorSetup ? <div className="two-factor-setup">
            <label><span>{ui(locale, "验证器密钥", "Authenticator secret")}</span><div className="copy-field"><input readOnly value={twoFactorSetup.secret} onFocus={(event) => event.currentTarget.select()} onClick={(event) => event.currentTarget.select()} /><button type="button" className={`secondary-btn compact copy-secret-btn ${twoFactorSecretCopied ? "copied" : ""}`} aria-live="polite" onClick={() => void copyTwoFactorSecret()}>{twoFactorSecretCopied ? <Check size={14} /> : <Copy size={14} />}{twoFactorSecretCopied ? ui(locale, "已复制", "Copied") : ui(locale, "复制密钥", "Copy secret")}</button></div></label>
            <p className="settings-hint">{ui(locale, "在 Google Authenticator、Aegis、2FAS 等验证器中手动输入该密钥，再填写当前 6 位验证码确认。", "Enter this secret manually in an authenticator such as Google Authenticator, Aegis or 2FAS, then confirm with the current 6-digit code.")}</p>
          </div> : null}
          {twoFactorStatus?.enabled ? <div className="two-factor-actions"><button type="button" className="danger-btn two-factor-disable-btn" disabled={busy} onClick={() => void disableTwoFactor()}>{ui(locale, "禁用两步验证", "Disable two-factor authentication")}</button></div> : <div className="two-factor-actions">
            {!twoFactorSetup ? <p className="settings-hint">{twoFactorStatus?.has_secret ? ui(locale, "已有未启用的密钥；重新生成后，旧密钥会失效。", "A disabled secret exists; regenerating invalidates the old one.") : ui(locale, "尚未生成两步验证密钥。", "No two-factor secret has been generated yet.")}</p> : null}
            <button type="button" className="secondary-btn two-factor-generate-btn" disabled={busy} onClick={() => void setupTwoFactor()}>{twoFactorStatus?.has_secret ? ui(locale, "重新生成密钥", "Regenerate secret") : ui(locale, "生成密钥", "Generate secret")}</button>
            {twoFactorSetup ? <button type="button" className="primary-btn" disabled={busy} onClick={() => void enableTwoFactor()}>{ui(locale, "启用两步验证", "Enable two-factor authentication")}</button> : null}
          </div>}
        </div>
        <div className="login-devices-panel">
          <div className="login-devices-head"><div><div className="section-subtitle">{ui(locale, "登录设备", "Signed-in devices")}</div><p className="settings-hint">{ui(locale, "踢下线后，对应设备的登录状态会立即失效。", "Signing out a device invalidates its session immediately.")}</p></div><button type="button" className="secondary-btn compact" disabled={Boolean(revokingSessionId)} onClick={() => void loadLoginSessions()}><RotateCw size={14} />{ui(locale, "刷新", "Refresh")}</button></div>
          {!loginSessionsLoaded ? <p className="settings-hint">{ui(locale, "正在读取登录设备...", "Loading devices...")}</p> : loginSessions.length ? <div className="login-device-list">{loginSessions.map((session) => <div className="login-device-row" key={session.id}><span className="login-device-icon"><MonitorSmartphone size={18} /></span><div className="login-device-copy"><div className="login-device-title"><strong>{describeLoginDevice(session.user_agent, locale)}</strong>{session.current ? <span className="login-device-current">{ui(locale, "当前设备", "Current device")}</span> : null}</div><span>{session.ip_address || ui(locale, "未知 IP", "Unknown IP")}</span><small title={session.user_agent}>{ui(locale, `最近活动 ${formatSessionTime(session.last_seen_at)} · 登录于 ${formatSessionTime(session.created_at)} · 到期 ${formatSessionTime(session.expires_at)}`, `Last active ${formatSessionTime(session.last_seen_at)} · Signed in ${formatSessionTime(session.created_at)} · Expires ${formatSessionTime(session.expires_at)}`)}</small></div>{!session.current ? <button type="button" className="danger-btn compact login-device-revoke" disabled={Boolean(revokingSessionId)} onClick={() => void revokeLoginSession(session)}><LogOut size={14} />{revokingSessionId === session.id ? ui(locale, "下线中", "Signing out") : ui(locale, "踢下线", "Sign out")}</button> : null}</div>)}</div> : <p className="settings-hint">{ui(locale, "暂无有效登录设备。", "No active devices.")}</p>}
        </div>
        <div className="form-grid"><Toggle label={ui(locale, "保护公开仪表盘", "Protect public dashboard")} checked={settings.turnstile_enabled} onChange={(value) => updateSettings("turnstile_enabled", value)} /><Toggle label={ui(locale, "保护管理员登录", "Protect admin sign-in")} checked={settings.turnstile_login_enabled} onChange={(value) => updateSettings("turnstile_login_enabled", value)} /></div>
        <div className="form-grid"><label><span>Turnstile Site Key</span><input autoComplete="off" type="password" value={settings.turnstile_site_key} onChange={(event) => updateSettings("turnstile_site_key", event.target.value)} /></label><label><span>Turnstile Secret Key</span><input autoComplete="off" type="password" value={settings.turnstile_secret_key} onChange={(event) => updateSettings("turnstile_secret_key", event.target.value)} /></label></div>
      </> : null}

      {tab === "data" ? <>
        <div className="section-head database-section-head"><div><h3>{ui(locale, "数据库维护", "Database maintenance")}</h3><span>{ui(locale, "备份包含节点、设置、历史、主题文件、任务及安全配置，请妥善保管。", "Backups include servers, settings, history, theme files, tasks and security configuration. Keep them safe.")}</span></div><div className="section-actions"><button type="button" className="secondary-btn compact" disabled={busy || database?.restart_required} onClick={() => void exportDatabaseBackup()}><Download size={15} />{ui(locale, "导出备份", "Export backup")}</button><button type="button" className="secondary-btn compact" disabled={busy || database?.restart_required} onClick={() => databaseRestoreInputRef.current?.click()}><Upload size={15} />{ui(locale, "恢复备份", "Restore backup")}</button><input ref={databaseRestoreInputRef} hidden type="file" accept=".zip,application/zip" onChange={(event) => { const file = event.currentTarget.files?.[0]; event.currentTarget.value = ""; if (file) void restoreDatabaseBackup(file); }} /></div></div>
        <div className="database-storage"><div><span>{ui(locale, "数据库大小", "Database size")}</span><strong>{database ? formatBytes(database.size_bytes) : ui(locale, "读取中...", "Loading...")}</strong>{database ? <small>{database.kind === "postgresql" ? "PostgreSQL" : "SQLite"}{database.reclaimable_bytes ? ui(locale, ` · 可回收 ${formatBytes(database.reclaimable_bytes)}`, ` · ${formatBytes(database.reclaimable_bytes)} reclaimable`) : ""}</small> : null}</div><button type="button" className="secondary-btn" disabled={busy || !database || database.restart_required} onClick={() => void reclaimDatabase()}><RotateCw size={15} />{ui(locale, "回收空间", "Reclaim space")}</button></div>
        <div className="database-migration">
          <div><div className="section-title"><ArrowRightLeft size={15} />{ui(locale, "数据库迁移", "Database migration")}</div><p className="settings-hint">{ui(locale, `将当前 ${database?.kind === "postgresql" ? "PostgreSQL" : "SQLite"} 数据复制到 ${database?.kind === "postgresql" ? "SQLite" : "PostgreSQL"}，完成后自动更新配置，重启服务后生效。`, `Copy data from the current ${database?.kind === "postgresql" ? "PostgreSQL" : "SQLite"} database to ${database?.kind === "postgresql" ? "SQLite" : "PostgreSQL"}; the config is updated automatically and takes effect after a restart.`)}</p></div>
          {database?.restart_required ? <div className="database-migration-ready" role="status" aria-live="polite"><CircleCheck size={20} /><div><strong>{ui(locale, "迁移完成，业务写入已暂停", "Migration complete; writes are paused")}</strong><span>{databaseMigrationResult ? ui(locale, `已复制 ${databaseMigrationResult.migrated_rows.toLocaleString()} 行，新数据库大小 ${formatBytes(databaseMigrationResult.size_bytes)}，请重启服务`, `Copied ${databaseMigrationResult.migrated_rows.toLocaleString()} rows; new database size ${formatBytes(databaseMigrationResult.size_bytes)}. Restart the service`) : ui(locale, "配置已更新，重启后切换到新数据库并恢复写入", "Config updated; restart to switch to the new database and resume writes")}</span></div><button type="button" className="primary-btn" disabled={busy} onClick={() => void restartAfterDatabaseMigration()}>{restarting ? <RotateCw className="spin" size={15} /> : <Power size={15} />}{restarting ? ui(locale, "正在重启", "Restarting") : ui(locale, "立即重启", "Restart now")}</button></div> : <div className="database-migration-form"><label><span>{ui(locale, "目标数据库 URL", "Target database URL")}</span><input required type="password" autoComplete="off" maxLength={2048} spellCheck={false} value={databaseMigrationUrl} onChange={(event) => setDatabaseMigrationUrl(event.target.value)} placeholder={database?.kind === "postgresql" ? "sqlite://nodeflare-migrated.db" : "postgres://user:password@127.0.0.1:5432/nodeflare?sslmode=prefer"} /></label><button className="primary-btn" disabled={busy || !databaseMigrationUrl.trim()}><ArrowRightLeft size={15} />{busy ? ui(locale, "迁移中", "Migrating") : ui(locale, "开始迁移", "Start migration")}</button></div>}
        </div>
        <div className="usage-section">
          <div className="usage-head"><div><div className="section-title"><Coins size={15} />{ui(locale, "每日汇率", "Daily exchange rates")}</div><p className="settings-hint">{exchangeRates ? `${exchangeRates.source} · ${exchangeRates.date || ui(locale, "等待首次更新", "awaiting first update")}${exchangeRates.stale ? ui(locale, " · 数据待更新", " · stale") : ""}` : ui(locale, "正在读取汇率快照", "Loading rate snapshot")}</p></div><div className="usage-head-actions">{exchangeRatesExpanded ? <button type="button" className="secondary-btn compact" disabled={busy || database?.restart_required} onClick={() => void refreshExchangeRates()}><RotateCw size={15} />{ui(locale, "立即更新", "Refresh now")}</button> : null}<button type="button" className="secondary-btn compact usage-toggle" aria-expanded={exchangeRatesExpanded} onClick={() => setExchangeRatesExpanded((expanded) => !expanded)}>{exchangeRatesExpanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}{exchangeRatesExpanded ? ui(locale, "收起", "Collapse") : ui(locale, "展开", "Expand")}</button></div></div>
          {exchangeRatesExpanded ? exchangeRates ? <div className="usage-table-wrap"><table className="usage-table"><thead><tr><th>{ui(locale, "币种", "Currency")}</th><th>{ui(locale, "1 CNY 可兑换", "1 CNY equals")}</th></tr></thead><tbody>{ASSET_CURRENCIES.filter((currency) => currency !== "CNY").map((currency) => <tr key={currency}><th scope="row">{currency}</th><td>{exchangeRates.rates[currency]?.toLocaleString(undefined, { maximumFractionDigits: 6 }) ?? "--"}</td></tr>)}</tbody></table></div> : <div className="usage-empty">{ui(locale, "尚未读取", "Not loaded")}</div> : null}
        </div>
      </> : null}

      {tab !== "data" ? <div className="form-actions"><button className="primary-btn" disabled={busy}><Save size={15} />{ui(locale, "保存设置", "Save settings")}</button></div> : null}
    </form>

  );
}
