import type { TelegramSettings, TelegramSettingsInput, WebhookPreset, WebhookSettingsInput, WebhookSettingsView } from "./types";
import { WEBHOOK_PRESETS, WEBHOOK_PREVIEW_VALUES, renderWebhookBody } from "./webhook";

export type NotificationChannel = "telegram" | WebhookPreset;
export const WEBHOOK_CHANNELS: WebhookPreset[] = ["bark", "discord", "slack", "wecom", "dingtalk", "feishu", "ntfy", "gotify", "custom"];
export const NOTIFICATION_CHANNELS: NotificationChannel[] = ["telegram", ...WEBHOOK_CHANNELS];
export interface SavedNotificationChannels {
  telegram: TelegramSettings | null;
  webhooks: WebhookSettingsView[];
}
export type WebhookDraft = Omit<WebhookSettingsInput, "enabled">;
export interface NotificationDraft {
  channels: NotificationChannel[];
  telegram: Omit<TelegramSettingsInput, "enabled">;
  webhooks: Record<WebhookPreset, WebhookDraft>;
}

export function webhookDraft(preset: WebhookPreset, saved?: WebhookSettingsView): WebhookDraft {
  const defaults = WEBHOOK_PRESETS[preset];
  return {
    preset, url: saved ? "" : defaults.url, headers: saved ? "" : defaults.headers,
    body: saved ? "" : defaults.body, clear_headers: false,
  };
}

export function webhookReady(draft: WebhookDraft, saved?: WebhookSettingsView): boolean {
  if (!(draft.url.trim() || saved?.url_configured) || !(draft.body.trim() || saved?.body_configured)) return false;
  try {
    if (draft.body.trim()) renderWebhookBody(draft.body, WEBHOOK_PREVIEW_VALUES);
    return true;
  } catch { return false; }
}

export function notificationDraft(saved: SavedNotificationChannels, enabled: boolean): NotificationDraft {
  const { enabled: telegramEnabled, ...telegram } = saved.telegram ?? {
    enabled: false, bot_token: "", chat_id: "", message_thread_id: null,
    template: "{{title}}\n\n服务器：{{server}}\n{{message}}\n时间：{{time}}",
  };
  return {
    channels: enabled ? [
      ...(telegramEnabled ? ["telegram" as const] : []),
      ...WEBHOOK_CHANNELS.filter((preset) => saved.webhooks.some((item) => item.preset === preset && item.enabled)),
    ] : [],
    telegram,
    webhooks: Object.fromEntries(WEBHOOK_CHANNELS.map((preset) => [preset, webhookDraft(preset, saved.webhooks.find((item) => item.preset === preset))])) as NotificationDraft["webhooks"],
  };
}

export function notificationUpdates(draft: NotificationDraft, saved: SavedNotificationChannels): {
  telegram: TelegramSettingsInput | null;
  webhooks: WebhookSettingsInput[];
} {
  return {
    telegram: draft.channels.includes("telegram") ? {
      ...draft.telegram, enabled: true, bot_token: draft.telegram.bot_token.trim(),
      chat_id: draft.telegram.chat_id.trim(), template: draft.telegram.template.trim(),
    } : saved.telegram ? { ...saved.telegram, enabled: false } : null,
    // Disabling a channel must not save an incomplete draft or clear its secrets.
    webhooks: WEBHOOK_CHANNELS.flatMap<WebhookSettingsInput>((preset) => draft.channels.includes(preset)
      ? [{ ...draft.webhooks[preset], preset, enabled: true }]
      : saved.webhooks.some((item) => item.preset === preset)
        ? [{ enabled: false, preset, url: "", headers: "", body: "", clear_headers: false }] : []),
  };
}
