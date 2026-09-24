import { describe, expect, test } from "bun:test";
import { localizedError, TRANSLATED_MESSAGES } from "./errors";

describe("localizedError", () => {
  test("returns the server text unchanged for Chinese dashboards", () => {
    expect(localizedError("用户名或密码错误", "zh-CN")).toBe("用户名或密码错误");
    expect(localizedError("用户名或密码错误", undefined)).toBe("用户名或密码错误");
  });

  test("translates known messages for English dashboards", () => {
    expect(localizedError("用户名或密码错误", "en")).toBe("Incorrect username or password");
    expect(localizedError("请选择 1 至 128 个节点并填写命令", "en"))
      .toBe("Select 1 to 128 servers and enter a command");
  });

  test("translates messages that embed runtime values", () => {
    expect(localizedError("登录尝试过多，请在 42 秒后重试", "en"))
      .toBe("Too many login attempts; retry in 42s");
    expect(localizedError("迁移失败：disk full", "en")).toBe("Migration failed: disk full");
    expect(localizedError("节点不存在: abc", "en")).toBe("Server not found: abc");
    expect(localizedError("Agent 协议不兼容，需要协议版本 2", "en"))
      .toBe("Incompatible Agent protocol; version 2 is required");
    expect(localizedError("恢复失败：备份缺少管理员登录设置", "en"))
      .toBe("Restore failed: 备份缺少管理员登录设置");
  });

  test("passes unknown messages through so new errors stay visible", () => {
    expect(localizedError("brand new server message", "en")).toBe("brand new server message");
    expect(localizedError("", "en")).toBe("");
  });

  test("never returns an empty translation for a known message", () => {
    for (const [source, translation] of Object.entries(TRANSLATED_MESSAGES)) {
      expect(translation.trim().length, source).toBeGreaterThan(0);
      expect(translation, source).not.toMatch(/[\u4e00-\u9fff]/);
    }
  });
});

describe("ApiError locale wiring", () => {
  test("uses the locale that was set from the bootstrap config", async () => {
    const { ApiError, setApiLocale } = await import("./api");
    setApiLocale("en");
    expect(new ApiError("节点不存在", 404).message).toBe("Server not found");
    // An unknown locale must not accidentally enable translation.
    setApiLocale("fr");
    expect(new ApiError("节点不存在", 404).message).toBe("节点不存在");
    setApiLocale("zh-CN");
    expect(new ApiError("节点不存在", 404).message).toBe("节点不存在");
  });
});
