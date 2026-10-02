import { describe, expect, test } from "bun:test";
import { NOTIFICATION_CHANNELS, WEBHOOK_CHANNELS, notificationDraft, notificationUpdates, webhookDraft, webhookReady, type SavedNotificationChannels } from "./notifications";
import { WEBHOOK_PRESETS } from "./webhook";

const saved: SavedNotificationChannels = {
  telegram: { enabled: true, bot_token: "********", chat_id: "********", message_thread_id: 10, template: "{{message}}" },
  webhooks: (["bark", "discord"] as const).map((preset) => ({ enabled: true, preset, url_configured: true, headers_configured: true, body_configured: true })),
};

describe("notification channels", () => {
  test("every service is directly selectable, with custom Webhook as a separate channel", () => {
    expect(NOTIFICATION_CHANNELS.slice(0, 3)).toEqual(["telegram", "bark", "discord"]);
    expect(NOTIFICATION_CHANNELS).not.toContain("webhook");
    expect(new Set(NOTIFICATION_CHANNELS).size).toBe(NOTIFICATION_CHANNELS.length);
    expect(Object.keys(WEBHOOK_PRESETS).sort()).toEqual([...WEBHOOK_CHANNELS].sort());
  });

  test("unconfigured channels start unselected and do not create empty records", () => {
    const empty = { telegram: null, webhooks: [] };
    const draft = notificationDraft(empty, true);
    expect(draft.channels).toEqual([]);
    expect(notificationUpdates(draft, empty)).toEqual(empty);
    expect(draft.webhooks.bark.body).toContain("YOUR_DEVICE_KEY");
    expect(draft.webhooks.discord.body).toContain("content");
  });

  test("loads Telegram, Bark and Discord simultaneously without reading back secrets", () => {
    const draft = notificationDraft(saved, true);
    expect(draft.channels).toEqual(["telegram", "bark", "discord"]);
    expect(draft.telegram.bot_token).toBe("********");
    expect(draft.telegram.chat_id).toBe("********");
    for (const preset of ["bark", "discord"] as const) {
      expect(draft.webhooks[preset]).toEqual({ preset, url: "", headers: "", body: "", clear_headers: false });
      expect(webhookReady(draft.webhooks[preset], saved.webhooks.find((item) => item.preset === preset))).toBe(true);
    }
    expect(draft.telegram).not.toHaveProperty("enabled");
  });

  test("a disabled master switch does not enable saved channels on opening", () => {
    const draft = notificationDraft(saved, false);
    expect(draft.channels).toEqual([]);
    expect(draft.telegram.message_thread_id).toBe(10);
    expect(draft.webhooks.bark.url).toBe("");
  });

  test("each channel's enabled flag determines the selected subset", () => {
    const channels = { ...saved, telegram: { ...saved.telegram!, enabled: false }, webhooks: saved.webhooks.map((item) => ({ ...item, enabled: item.preset === "discord" })) };
    expect(notificationDraft(channels, true).channels).toEqual(["discord"]);
  });

  test("deselecting channels preserves saved values and ignores incomplete hidden edits", () => {
    const draft = notificationDraft(saved, true);
    draft.channels = [];
    draft.telegram.bot_token = "";
    draft.telegram.template = "unfinished template";
    draft.webhooks.bark = { preset: "bark", url: "incomplete", headers: "", body: "bad JSON", clear_headers: true };
    const updates = notificationUpdates(draft, saved);
    expect(updates.telegram).toEqual({ ...saved.telegram!, enabled: false });
    expect(updates.webhooks).toEqual((["bark", "discord"] as const).map((preset) => ({ enabled: false, preset, url: "", headers: "", body: "", clear_headers: false })));
  });

  test("re-enabling reuses each channel's own saved credentials", () => {
    const draft = notificationDraft(saved, false);
    draft.channels = ["telegram", "bark", "discord"];
    const updates = notificationUpdates(draft, saved);
    expect(updates.telegram).toEqual(saved.telegram);
    expect(updates.webhooks).toEqual((["bark", "discord"] as const).map((preset) => ({ enabled: true, preset, url: "", headers: "", body: "", clear_headers: false })));
  });

  test("only selected channels save edits, without overwriting another service", () => {
    const draft = notificationDraft(saved, true);
    draft.channels = ["telegram", "discord", "custom"];
    draft.telegram = { bot_token: " 123:token ", chat_id: " -1001 ", message_thread_id: null, template: " {{title}} " };
    draft.webhooks.discord = { preset: "discord", url: "https://discord.example/new", headers: "", body: '{"content":"{{message}}"}', clear_headers: true };
    draft.webhooks.custom.url = "https://custom.example/new";
    const updates = notificationUpdates(draft, saved);
    expect(updates.telegram).toEqual({ enabled: true, bot_token: "123:token", chat_id: "-1001", message_thread_id: null, template: "{{title}}" });
    expect(updates.webhooks.find((item) => item.preset === "bark")).toEqual({ enabled: false, preset: "bark", url: "", headers: "", body: "", clear_headers: false });
    expect(updates.webhooks.find((item) => item.preset === "discord")).toEqual({ ...draft.webhooks.discord, enabled: true });
    expect(updates.webhooks.find((item) => item.preset === "custom")?.url).toBe("https://custom.example/new");
    expect(updates.webhooks).toHaveLength(3);
  });

  test("unsaved channels need valid fields and templates", () => {
    const draft = webhookDraft("discord");
    expect(webhookReady(draft)).toBe(false);
    draft.url = "https://example.com/discord";
    expect(webhookReady(draft)).toBe(true);
    draft.body = '{"content":{{message}}}';
    expect(webhookReady(draft)).toBe(false);
  });
});
