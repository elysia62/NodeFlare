import { Bell, CheckCircle2, Send, Save, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { ui, type UiLocale } from "../locale";
import type { WebhookPreset, WebhookSettingsInput, WebhookSettingsView } from "../types";
import { WEBHOOK_PRESETS, WEBHOOK_PREVIEW_VALUES, renderWebhookBody } from "../webhook";
import { Toggle } from "./admin/controls";

const emptySettings: WebhookSettingsInput = { enabled: true, preset: "custom", url: "", headers: "", body: WEBHOOK_PRESETS.custom.body, clear_headers: false };

export function WebhookSettings({ locale, onError, onNotice }: {
  locale: UiLocale | string | undefined;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [settings, setSettings] = useState<WebhookSettingsInput>(emptySettings);
  const [saved, setSaved] = useState<WebhookSettingsView | null>(null);
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    try {
      const result = await api.webhookSettings();
      setSaved(result.webhook);
      setSettings(result.webhook ? { ...emptySettings, enabled: result.webhook.enabled, preset: result.webhook.preset, body: "" } : emptySettings);
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "读取 Webhook 配置失败", "Failed to load Webhook settings"));
    }
  }, [onError, locale]);
  useEffect(() => { void load(); }, [load]);

  function update<K extends keyof WebhookSettingsInput>(key: K, value: WebhookSettingsInput[K]) {
    setSettings(current => ({ ...current, [key]: value }));
  }

  function choosePreset(preset: WebhookPreset) {
    const next = WEBHOOK_PRESETS[preset];
    setSettings(current => ({ ...current, preset, url: next.url, headers: next.headers, body: next.body, clear_headers: true }));
  }

  async function perform(action: () => Promise<void>, notice: string, reload: boolean) {
    setBusy(true); onError("");
    try {
      await action();
      if (reload) await load();
      onNotice(notice);
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "Webhook 操作失败", "Webhook operation failed"));
    } finally { setBusy(false); }
  }

  const preview = useMemo(() => {
    if (!settings.body.trim()) return { body: "", valid: true };
    try { return { body: renderWebhookBody(settings.body, WEBHOOK_PREVIEW_VALUES), valid: true }; }
    catch { return { body: ui(locale, "请求体不是有效 JSON；请把占位符放在引号内。", "Invalid JSON body; put placeholders inside quotes."), valid: false }; }
  }, [settings.body, locale]);
  const keepSaved = saved?.preset === settings.preset;
  const ready = Boolean((settings.url.trim() || (keepSaved && saved?.url_configured)) && (settings.body.trim() || (keepSaved && saved?.body_configured)) && preview.valid);
  const kept = ui(locale, "已保存；留空不修改", "Saved; leave empty to keep");

  return <section className="notification-settings" aria-labelledby="webhook-settings-title">
    <header className="notification-settings-header">
      <div className="notification-settings-title"><span className="notification-mark"><Bell size={18} /></span>
        <div><h3 id="webhook-settings-title">Webhook</h3><span className={`notification-state ${saved ? "configured" : ""}`}>{saved ? <CheckCircle2 size={13} /> : null}{saved ? ui(locale, "已配置", "Configured") : ui(locale, "未配置", "Not configured")}</span></div>
      </div>
    </header>
    <Toggle label={ui(locale, "启用 Webhook", "Enable Webhook")} checked={settings.enabled} onChange={value => update("enabled", value)} />
    <label><span>{ui(locale, "常用服务预设", "Service preset")}</span><select disabled={busy} value={settings.preset} onChange={event => choosePreset(event.target.value as WebhookPreset)}>
      {(Object.keys(WEBHOOK_PRESETS) as WebhookPreset[]).map(key => <option key={key} value={key}>{key === "custom" ? ui(locale, "自定义 Webhook", "Custom Webhook") : WEBHOOK_PRESETS[key].name}</option>)}
    </select></label>
    <p className="settings-hint">{ui(locale, "选择预设会填入示例；请替换 YOUR_KEY、YOUR_TOKEN、YOUR_DEVICE_KEY 等占位值。", "Presets fill in examples; replace placeholders such as YOUR_KEY, YOUR_TOKEN and YOUR_DEVICE_KEY.")}</p>
    <label><span>{ui(locale, "推送 URL", "Push URL")}</span><input type="password" maxLength={2048} autoComplete="off" value={settings.url} onChange={event => update("url", event.target.value)} placeholder={keepSaved && saved?.url_configured ? kept : "https://example.com/webhook"} /></label>
    <label><span>{ui(locale, "请求头（可选，一行一个 Name: value）", "Headers (optional, one Name: value per line)")}</span><textarea rows={3} maxLength={8192} autoComplete="off" value={settings.headers} onChange={event => update("headers", event.target.value)} placeholder={settings.clear_headers ? ui(locale, "保存后清除请求头", "Headers will be cleared on save") : saved?.headers_configured ? kept : "Authorization: Bearer ..."} /></label>
    {keepSaved && saved?.headers_configured ? <div className="notification-actions"><button type="button" className="secondary-btn compact" disabled={busy} onClick={() => setSettings(current => ({ ...current, headers: "", clear_headers: !current.clear_headers }))}>{settings.clear_headers ? ui(locale, "保留已保存请求头", "Keep saved headers") : ui(locale, "清除请求头（保存后生效）", "Clear headers on save")}</button></div> : null}
    <label className="notification-template"><span>{ui(locale, "JSON 请求体模板", "JSON body template")}</span><textarea rows={8} maxLength={16384} autoComplete="off" spellCheck={false} value={settings.body} onChange={event => update("body", event.target.value)} placeholder={keepSaved && saved?.body_configured ? kept : WEBHOOK_PRESETS.custom.body} /></label>
    <p className="settings-hint">{ui(locale, "支持 {{title}}、{{message}}、{{node}} / {{server}}、{{event}}、{{site}}、{{time}}，占位符需放在 JSON 字符串引号内。", "Supports {{title}}, {{message}}, {{node}} / {{server}}, {{event}}, {{site}} and {{time}}; place placeholders inside JSON string quotes.")}</p>
    {preview.body ? <details className="webhook-preview" open={!preview.valid}><summary>{ui(locale, "请求体预览", "Body preview")}</summary><pre className={preview.valid ? "" : "form-error"}>{preview.body}</pre></details> : null}
    <p className="settings-hint">{ui(locale, "URL、请求头和请求体可能包含密钥，保存后不回读；留空表示保留。以 POST 发送，默认 JSON，不跟随地址跳转。测试使用已保存的配置。", "URL, headers and body may contain secrets and are never returned after saving; empty fields keep current values. Sends POST with JSON by default and does not follow redirects. Tests use saved settings.")}</p>
    {settings.preset === "dingtalk" || settings.preset === "feishu" ? <p className="settings-hint">{ui(locale, "机器人关键词请包含 NodeFlare；暂不支持动态签名。", "Include NodeFlare in the bot's keywords; dynamic signatures are not supported.")}</p> : null}
    <div className="notification-actions">
      {saved ? <button type="button" className="secondary-btn danger" disabled={busy} onClick={() => void perform(api.clearWebhookSettings, ui(locale, "Webhook 配置已清除", "Webhook settings cleared"), true)}><Trash2 size={15} />{ui(locale, "清除 Webhook", "Clear Webhook")}</button> : null}
      <button type="button" className="secondary-btn" disabled={busy || !saved} onClick={() => void perform(api.testWebhook, ui(locale, "Webhook 测试消息已发送", "Webhook test message sent"), false)}><Send size={15} />{ui(locale, "发送测试", "Send test")}</button>
      <button type="button" className="primary-btn" disabled={busy || !ready} onClick={() => void perform(() => api.saveWebhookSettings(settings), ui(locale, "Webhook 配置已保存", "Webhook settings saved"), true)}><Save size={15} />{ui(locale, "保存 Webhook", "Save Webhook")}</button>
    </div>
  </section>;
}
