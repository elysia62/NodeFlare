import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("./NotificationSettings.tsx", import.meta.url), "utf8");

describe("notification secret fields", () => {
  test("secrets and the push URL can be revealed instead of staying permanently masked", () => {
    // A bare type="password" hides the value with no way to check what was typed.
    expect(source).not.toContain('type="password"');
    expect(source.match(/<PasswordInput/g)?.length).toBe(3);
  });

  test("keeps the placeholder and bot-keyword hints without the delivery-mechanics blurb", () => {
    expect(source).toContain("YOUR_KEY、YOUR_TOKEN、YOUR_DEVICE_KEY");
    expect(source).toContain("机器人关键词请包含 NodeFlare");
    expect(source).not.toContain("不跟随地址跳转");
    expect(source).not.toContain("does not follow redirects");
  });
});
