import { Bell, CheckCircle2, RotateCw, Save, Send, Trash2 } from "lucide-react";
import { type FormEvent, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { ui, type UiLocale } from "../locale";
import { NOTIFICATION_CHANNELS, notificationDraft, notificationUpdates, webhookDraft, webhookReady, type NotificationChannel, type NotificationDraft, type SavedNotificationChannels, type WebhookDraft } from "../notifications";
import type { AdminServer, Settings, WebhookPreset, WebhookSettingsView } from "../types";
import { WEBHOOK_PRESETS, WEBHOOK_PREVIEW_VALUES, renderWebhookBody } from "../webhook";
import { AlertRuleManager } from "./AlertRuleManager";
import { PasswordInput } from "./PasswordInput";
import { Select } from "./admin/Select";

const emptySaved: SavedNotificationChannels = { telegram: null, webhooks: [] };

function channelName(channel: NotificationChannel, locale: UiLocale | string | undefined): string {
  if (channel === "telegram") return "Telegram";
  return channel === "custom" ? ui(locale, "自定义 Webhook", "Custom Webhook") : WEBHOOK_PRESETS[channel].name;
}

export function NotificationSettings({ locale, settings, servers, updateSettings, onSaved, onError, onNotice }: {
  locale: UiLocale | string | undefined;
  settings: Settings;
  servers: AdminServer[];
  updateSettings: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  onSaved: (settings: Settings) => void;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [saved, setSaved] = useState<SavedNotificationChannels | null>(null);
  const [draft, setDraft] = useState(() => notificationDraft(emptySaved, false));
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [reload, setReload] = useState(0);
  useEffect(() => {
    let active = true;
    setLoading(true);
    setSaved(null);
    Promise.all([api.telegramSettings(), api.webhookSettings()]).then(([telegram, webhook]) => {
      if (!active) return;
      const result = { telegram: telegram.telegram, webhooks: webhook.webhooks };
      setSaved(result);
      setDraft(notificationDraft(result, settings.notification_enabled));
    }).catch((reason) => {
      if (active) onError(reason instanceof Error ? reason.message : ui(locale, "读取通知配置失败", "Failed to load notification settings"));
    }).finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [locale, onError, reload, settings.notification_enabled]);

  function updateTelegram<K extends keyof NotificationDraft["telegram"]>(key: K, value: NotificationDraft["telegram"][K]) {
    setDraft((current) => ({ ...current, telegram: { ...current.telegram, [key]: value } }));
  }
  function updateWebhook(preset: WebhookPreset, changes: Partial<WebhookDraft>) {
    setDraft((current) => ({ ...current, webhooks: { ...current.webhooks, [preset]: { ...current.webhooks[preset], ...changes } } }));
  }

  const { telegram, webhooks, channels } = draft;
  const ready = channels.every((channel) => channel === "telegram"
    ? Boolean(telegram.bot_token.trim() && telegram.chat_id.trim() && telegram.template.trim())
    : webhookReady(webhooks[channel], saved?.webhooks.find((item) => item.preset === channel)));
  const locked = loading || busy || !saved;

  async function save(event: FormEvent) {
    event.preventDefault();
    if (locked || !ready || !saved) return;
    setBusy(true); onError(""); onNotice("");
    try {
      const input = notificationUpdates(draft, saved);
      const writes: Array<{ name: string; run: () => Promise<void> }> = [];
      if (input.telegram) {
        const value = input.telegram;
        writes.push({ name: "Telegram", run: async () => {
          await api.saveTelegramSettings(value);
          const { telegram } = await api.telegramSettings();
          setSaved((current) => current && { ...current, telegram });
          setDraft((current) => ({ ...current, telegram: notificationDraft({ ...emptySaved, telegram }, false).telegram }));
        } });
      }
      for (const value of input.webhooks) {
        writes.push({ name: channelName(value.preset, locale), run: async () => {
          const webhook = await api.saveWebhookSettings(value);
          setSaved((current) => current && { ...current, webhooks: [...current.webhooks.filter((item) => item.preset !== value.preset), webhook] });
          setDraft((current) => ({ ...current, webhooks: { ...current.webhooks, [value.preset]: webhookDraft(value.preset, webhook) } }));
        } });
      }
      const results = await Promise.allSettled(writes.map((write) => write.run()));
      const failures = results.flatMap((result, index) => result.status === "rejected"
        ? [`${writes[index].name}: ${result.reason instanceof Error ? result.reason.message : ui(locale, "保存失败", "Save failed")}`] : []);
      if (failures.length) throw new Error(`${ui(locale, "通知设置未全部保存，请重试。", "Notification settings were not fully saved; retry.")} ${failures.join("；")}`);
      const result = await api.saveSettings({
        notification_enabled: channels.length > 0,
        offline_alert_minutes: settings.offline_alert_minutes,
        expiry_alert_days: settings.expiry_alert_days,
        traffic_alert_percentage: settings.traffic_alert_percentage,
      });
      onSaved(result);
      onNotice(ui(locale, "通知设置已保存", "Notification settings saved"));
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "保存通知设置失败", "Failed to save notification settings"));
    } finally { setBusy(false); }
  }

  async function perform(action: () => Promise<void>, notice: string) {
    if (locked) return;
    setBusy(true); onError(""); onNotice("");
    try { await action(); onNotice(notice); }
    catch (reason) { onError(reason instanceof Error ? reason.message : ui(locale, "通知操作失败", "Notification operation failed")); }
    finally { setBusy(false); }
  }

  async function clearWebhook(preset: WebhookPreset) {
    await api.clearWebhookSettings(preset);
    setSaved((current) => current && { ...current, webhooks: current.webhooks.filter((item) => item.preset !== preset) });
    setDraft((current) => ({ ...current, channels: current.channels.filter((channel) => channel !== preset), webhooks: { ...current.webhooks, [preset]: webhookDraft(preset) } }));
  }

  return <>
    <form className="settings-form notification-form" onSubmit={(event) => void save(event)}>
      <div className="section-title"><Bell size={15} />{ui(locale, "通知", "Notifications")}</div>
      {loading ? <p className="settings-hint" role="status">{ui(locale, "正在读取通知配置…", "Loading notification settings…")}</p> : !saved ? <button type="button" className="secondary-btn" onClick={() => setReload((value) => value + 1)}><RotateCw size={15} />{ui(locale, "重新读取通知配置", "Reload notification settings")}</button> : null}
      <fieldset className="notification-form-fields" disabled={locked}>
        <Select multiple label={ui(locale, "启用通知渠道", "Enabled notification channels")}
          value={channels} onChange={(value) => setDraft((current) => ({ ...current, channels: value as NotificationChannel[] }))}
          placeholder={ui(locale, "未启用通知渠道", "No notification channels enabled")} disabled={locked}
          options={NOTIFICATION_CHANNELS.map((channel) => ({
            value: channel, label: channelName(channel, locale),
            description: (channel === "telegram" ? saved?.telegram : saved?.webhooks.some((item) => item.preset === channel))
              ? ui(locale, "已配置", "Configured") : ui(locale, "未配置", "Not configured"),
          }))} />
        <div className="form-grid three">
          <label><span>{ui(locale, "离线告警延迟（分钟）", "Offline alert delay (min)")}</span><input type="number" required min="2" max="1440" value={settings.offline_alert_minutes} onChange={(event) => updateSettings("offline_alert_minutes", Number(event.target.value))} /></label>
          <label><span>{ui(locale, "到期提醒（天）", "Expiry reminder (days)")}</span><input type="number" required min="0" max="365" value={settings.expiry_alert_days} onChange={(event) => updateSettings("expiry_alert_days", Number(event.target.value))} /></label>
          <label><span>{ui(locale, "流量提醒起始阈值（%）", "Traffic alert threshold (%)")}</span><input type="number" required min="50" max="100" value={settings.traffic_alert_percentage} onChange={(event) => updateSettings("traffic_alert_percentage", Number(event.target.value))} /></label>
        </div>
        <p className="settings-hint">{ui(locale, "流量达到起始阈值后，每增加 5 个百分点生成一次事件，最多提醒到 100%。", "After the threshold is reached, an event is created for every additional 5 percentage points, up to 100%.")}</p>

        {channels.includes("telegram") ? <section className="notification-settings" data-channel="telegram" aria-labelledby="telegram-settings-title">
          <header className="notification-settings-header"><h3 id="telegram-settings-title">Telegram</h3><span className={`notification-state ${saved?.telegram ? "configured" : ""}`}>{saved?.telegram ? <CheckCircle2 size={13} /> : null}{saved?.telegram ? ui(locale, "已配置", "Configured") : ui(locale, "未配置", "Not configured")}</span></header>
          <div className="notification-fields">
            <label className="notification-token-field"><span>Bot Token</span><PasswordInput locale={locale} required maxLength={512} autoComplete="off" value={telegram.bot_token} onChange={(event) => updateTelegram("bot_token", event.target.value)} placeholder="123456789:AA..." /></label>
            <label><span>Chat ID</span><PasswordInput locale={locale} required maxLength={128} autoComplete="off" value={telegram.chat_id} onChange={(event) => updateTelegram("chat_id", event.target.value)} placeholder="-1001234567890" /></label>
            <label><span>{ui(locale, "话题 ID（可选）", "Topic ID (optional)")}</span><input type="number" min="1" value={telegram.message_thread_id ?? ""} onChange={(event) => updateTelegram("message_thread_id", event.target.value ? Number(event.target.value) : null)} /></label>
          </div>
          <label className="notification-template"><span>{ui(locale, "消息模板", "Message template")}</span><textarea required rows={5} maxLength={4000} value={telegram.template} onChange={(event) => updateTelegram("template", event.target.value)} /></label>
          <div className="notification-actions"><button type="button" className="secondary-btn" disabled={!saved?.telegram} onClick={() => void perform(api.testTelegram, ui(locale, "Telegram 测试消息已发送", "Telegram test message sent"))}><Send size={15} />{ui(locale, "测试 Telegram", "Test Telegram")}</button></div>
        </section> : null}

        {NOTIFICATION_CHANNELS.filter((channel): channel is WebhookPreset => channel !== "telegram" && channels.includes(channel)).map((preset) =>
          <WebhookFields key={preset} locale={locale} value={webhooks[preset]} saved={saved?.webhooks.find((item) => item.preset === preset)}
            onChange={(changes) => updateWebhook(preset, changes)}
            onTest={() => void perform(() => api.testWebhook(preset), ui(locale, `${channelName(preset, locale)} 测试消息已发送`, `${channelName(preset, locale)} test message sent`))}
            onClear={() => void perform(() => clearWebhook(preset), ui(locale, `${channelName(preset, locale)} 配置已清除`, `${channelName(preset, locale)} settings cleared`))} />)}
      </fieldset>
      <div className="form-actions"><button className="primary-btn" disabled={locked || !ready}><Save size={15} />{ui(locale, "保存设置", "Save settings")}</button></div>
    </form>
    <div className="admin-section notification-rules"><AlertRuleManager locale={locale} servers={servers} onError={onError} onNotice={onNotice} /></div>
  </>;
}

function WebhookFields({ locale, value, saved, onChange, onTest, onClear }: {
  locale: UiLocale | string | undefined;
  value: WebhookDraft;
  saved: WebhookSettingsView | undefined;
  onChange: (changes: Partial<WebhookDraft>) => void;
  onTest: () => void;
  onClear: () => void;
}) {
  const name = channelName(value.preset, locale);
  const titleId = `notification-${value.preset}-title`;
  const kept = ui(locale, "已保存；留空不修改", "Saved; leave empty to keep");
  const preview = useMemo(() => {
    if (!value.body.trim()) return { body: "", valid: true };
    try { return { body: renderWebhookBody(value.body, WEBHOOK_PREVIEW_VALUES), valid: true }; }
    catch { return { body: ui(locale, "请求体不是有效 JSON；请把占位符放在引号内。", "Invalid JSON body; put placeholders inside quotes."), valid: false }; }
  }, [value.body, locale]);
  return <section className="notification-settings" data-channel={value.preset} aria-labelledby={titleId}>
    <header className="notification-settings-header"><h3 id={titleId}>{name}</h3><span className={`notification-state ${saved ? "configured" : ""}`}>{saved ? <CheckCircle2 size={13} /> : null}{saved ? ui(locale, "已配置", "Configured") : ui(locale, "未配置", "Not configured")}</span></header>
    {!saved && value.preset !== "custom" ? <p className="settings-hint">{ui(locale, "已填入此渠道的请求示例，请替换 YOUR_KEY、YOUR_TOKEN、YOUR_DEVICE_KEY 等占位值。", "This channel's request example is prefilled; replace placeholders such as YOUR_KEY, YOUR_TOKEN and YOUR_DEVICE_KEY.")}</p> : null}
    <label><span>{ui(locale, "推送 URL", "Push URL")}</span><PasswordInput locale={locale} required={!saved?.url_configured} maxLength={2048} autoComplete="off" value={value.url} onChange={(event) => onChange({ url: event.target.value })} placeholder={saved?.url_configured ? kept : "https://example.com/webhook"} /></label>
    <label><span>{ui(locale, "请求头（可选，一行一个 Name: value）", "Headers (optional, one Name: value per line)")}</span><textarea rows={3} maxLength={8192} autoComplete="off" value={value.headers} onChange={(event) => onChange({ headers: event.target.value })} placeholder={value.clear_headers ? ui(locale, "保存后清除请求头", "Headers will be cleared on save") : saved?.headers_configured ? kept : "Authorization: Bearer ..."} /></label>
    {saved?.headers_configured ? <div className="notification-actions"><button type="button" className="secondary-btn compact" onClick={() => onChange({ headers: "", clear_headers: !value.clear_headers })}>{value.clear_headers ? ui(locale, "保留已保存请求头", "Keep saved headers") : ui(locale, "清除请求头（保存后生效）", "Clear headers on save")}</button></div> : null}
    <label className="notification-template"><span>{ui(locale, "JSON 请求体模板", "JSON body template")}</span><textarea required={!saved?.body_configured} rows={6} maxLength={16384} autoComplete="off" spellCheck={false} value={value.body} onChange={(event) => onChange({ body: event.target.value })} placeholder={saved?.body_configured ? kept : WEBHOOK_PRESETS[value.preset].body} /></label>
    <p className="settings-hint">{ui(locale, "支持 {{title}}、{{message}}、{{node}} / {{server}}、{{event}}、{{site}}、{{time}}，占位符需放在 JSON 字符串引号内。", "Supports {{title}}, {{message}}, {{node}} / {{server}}, {{event}}, {{site}} and {{time}}; place placeholders inside JSON string quotes.")}</p>
    {preview.body ? <details className="webhook-preview" open={!preview.valid}><summary>{ui(locale, "请求体预览", "Body preview")}</summary><pre className={preview.valid ? "" : "form-error"}>{preview.body}</pre></details> : null}
    {value.preset === "dingtalk" || value.preset === "feishu" ? <p className="settings-hint">{ui(locale, "机器人关键词请包含 NodeFlare；暂不支持动态签名。", "Include NodeFlare in the bot's keywords; dynamic signatures are not supported.")}</p> : null}
    <div className="notification-actions">
      {saved ? <button type="button" className="secondary-btn danger" onClick={onClear}><Trash2 size={15} />{ui(locale, `清除 ${name}`, `Clear ${name}`)}</button> : null}
      <button type="button" className="secondary-btn" disabled={!saved} onClick={onTest}><Send size={15} />{ui(locale, `测试 ${name}`, `Test ${name}`)}</button>
    </div>
  </section>;
}
