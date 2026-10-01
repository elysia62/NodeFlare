import type { WebhookPreset } from "./types";

const combined = "{{title}}\n节点：{{node}}\n{{message}}";
const json = (value: unknown) => JSON.stringify(value, null, 2);
export const WEBHOOK_PRESETS: Record<WebhookPreset, { name: string; url: string; headers: string; body: string }> = {
  custom: { name: "Webhook", url: "", headers: "", body: json({ event: "{{event}}", node: "{{node}}", title: "{{title}}", message: "{{message}}", site: "{{site}}", time: "{{time}}" }) },
  bark: { name: "Bark", url: "https://api.day.app/push", headers: "", body: json({ device_key: "YOUR_DEVICE_KEY", title: "{{title}}", subtitle: "{{node}}", body: "{{message}}", group: "NodeFlare" }) },
  discord: { name: "Discord", url: "", headers: "", body: json({ content: combined }) },
  slack: { name: "Slack", url: "", headers: "", body: json({ text: combined }) },
  wecom: { name: "企业微信 / WeCom", url: "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=YOUR_KEY", headers: "", body: json({ msgtype: "text", text: { content: combined } }) },
  dingtalk: { name: "钉钉 / DingTalk", url: "https://oapi.dingtalk.com/robot/send?access_token=YOUR_TOKEN", headers: "", body: json({ msgtype: "text", text: { content: `NodeFlare\n${combined}` } }) },
  feishu: { name: "飞书 / Feishu", url: "", headers: "", body: json({ msg_type: "text", content: { text: `NodeFlare\n${combined}` } }) },
  ntfy: { name: "ntfy", url: "https://ntfy.sh", headers: "", body: json({ topic: "YOUR_TOPIC", title: "{{title}}", message: "{{message}}" }) },
  gotify: { name: "Gotify", url: "", headers: "X-Gotify-Key: YOUR_APP_TOKEN", body: json({ title: "{{title}}", message: "{{message}}" }) },
};

export const WEBHOOK_PREVIEW_VALUES: Record<string, string> = {
  title: '服务器离线：节点 "A"', node: '节点 "A"', server: '节点 "A"',
  message: '超过 2 分钟未收到 Agent 上报\n路径：C:\\data', event: "offline", site: "NodeFlare", time: "2026-10-01 12:00:00 UTC",
};

export function renderWebhookBody(template: string, values: Record<string, string>): string {
  const rendered = template.replace(/\{\{([^{}]*)\}\}/g, (token, key: string) =>
    Object.hasOwn(values, key) ? JSON.stringify(values[key]).slice(1, -1) : token);
  return JSON.stringify(JSON.parse(rendered), null, 2);
}
