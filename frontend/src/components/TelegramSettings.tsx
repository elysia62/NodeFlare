import { CheckCircle2, Send, Save } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import { ui, type UiLocale } from "../locale";
import type { TelegramSettingsInput } from "../types";

const defaultTemplate = "{{title}}\n\n服务器：{{server}}\n{{message}}\n时间：{{time}}";
const emptySettings: TelegramSettingsInput = {
  bot_token: "",
  chat_id: "",
  message_thread_id: null,
  template: defaultTemplate,
};

export function TelegramSettings({ locale, onError, onNotice }: {
  locale: UiLocale | string | undefined;
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [settings, setSettings] = useState<TelegramSettingsInput>(emptySettings);
  const [configured, setConfigured] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const result = await api.telegramSettings();
      if (result.telegram) {
        setSettings({
          bot_token: result.telegram.bot_token,
          chat_id: result.telegram.chat_id,
          message_thread_id: result.telegram.message_thread_id,
          template: result.telegram.template,
        });
        setConfigured(true);
      } else {
        setSettings(emptySettings);
        setConfigured(false);
      }
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "读取 Telegram 配置失败", "Failed to load Telegram settings"));
    }
  }, [onError]);

  useEffect(() => { void load(); }, [load]);

  function update<K extends keyof TelegramSettingsInput>(key: K, value: TelegramSettingsInput[K]) {
    setSettings((current) => ({ ...current, [key]: value }));
  }

  async function save() {
    setBusy(true);
    onError("");
    try {
      await api.saveTelegramSettings({
        ...settings,
        bot_token: settings.bot_token.trim(),
        chat_id: settings.chat_id.trim(),
        template: settings.template.trim(),
      });
      await load();
      onNotice(ui(locale, "Telegram 配置已保存", "Telegram settings saved"));
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "保存 Telegram 配置失败", "Failed to save Telegram settings"));
    } finally {
      setBusy(false);
    }
  }

  async function test() {
    setBusy(true);
    onError("");
    try {
      await api.testTelegram();
      onNotice(ui(locale, "Telegram 测试消息已发送", "Telegram test message sent"));
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "Telegram 测试消息发送失败", "Failed to send the Telegram test message"));
    } finally {
      setBusy(false);
    }
  }

  const ready = Boolean(settings.bot_token.trim() && settings.chat_id.trim() && settings.template.trim());

  return <section className="telegram-settings" aria-labelledby="telegram-settings-title">
    <header className="telegram-settings-header">
      <div className="telegram-settings-title">
        <span className="telegram-mark"><Send size={18} /></span>
        <div><h3 id="telegram-settings-title">Telegram</h3><span className={`telegram-state ${configured ? "configured" : ""}`}>{configured ? <CheckCircle2 size={13} /> : null}{configured ? ui(locale, "已配置", "Configured") : ui(locale, "未配置", "Not configured")}</span></div>
      </div>
    </header>
    <div className="telegram-fields">
      <label className="telegram-token-field"><span>Bot Token</span><input type="password" autoComplete="off" value={settings.bot_token} onChange={(event) => update("bot_token", event.target.value)} placeholder="123456789:AA..." /></label>
      <label><span>Chat ID</span><input type="password" autoComplete="off" value={settings.chat_id} onChange={(event) => update("chat_id", event.target.value)} placeholder="-1001234567890" /></label>
      <label><span>{ui(locale, "话题 ID（可选）", "Topic ID (optional)")}</span><input type="number" min="1" value={settings.message_thread_id ?? ""} onChange={(event) => update("message_thread_id", event.target.value ? Number(event.target.value) : null)} /></label>
    </div>
    <label className="telegram-template"><span>{ui(locale, "消息模板", "Message template")}</span><textarea rows={5} maxLength={4000} value={settings.template} onChange={(event) => update("template", event.target.value)} /></label>
    <div className="telegram-actions">
      <button type="button" className="secondary-btn" disabled={busy || !configured} onClick={() => void test()}><Send size={15} />{ui(locale, "发送测试", "Send test")}</button>
      <button type="button" className="primary-btn" disabled={busy || !ready} onClick={() => void save()}><Save size={15} />{ui(locale, "保存 Telegram", "Save Telegram")}</button>
    </div>
  </section>;
}
