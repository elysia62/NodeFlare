import { describe, expect, test } from "bun:test";
import { WEBHOOK_PRESETS, WEBHOOK_PREVIEW_VALUES, renderWebhookBody } from "./webhook";

describe("Webhook templates", () => {
  test("presets remain valid JSON for quotes, backslashes and multi-line messages", () => {
    const values: Record<string, string> = { ...WEBHOOK_PREVIEW_VALUES, message: 'first line\n"quoted"\\path\n{{event}}' };
    for (const preset of Object.values(WEBHOOK_PRESETS)) {
      const rendered = renderWebhookBody(preset.body, values);
      const body = JSON.parse(rendered);
      expect(body).toBeObject();
      expect(rendered).toContain("{{event}}");
    }
    const bark = JSON.parse(renderWebhookBody(WEBHOOK_PRESETS.bark.body, values));
    expect(bark.body).toBe(values.message);
    expect(bark.title).toBe(values.title);
  });

  test("rejects unquoted placeholders and keeps unknown placeholders as written", () => {
    expect(() => renderWebhookBody('{"message":{{message}}}', WEBHOOK_PREVIEW_VALUES)).toThrow();
    expect(JSON.parse(renderWebhookBody('{"node":"{{server}}","other":"{{unknown}}"}', WEBHOOK_PREVIEW_VALUES)))
      .toEqual({ node: WEBHOOK_PREVIEW_VALUES.node, other: "{{unknown}}" });
  });
});
