import { describe, expect, test } from "bun:test";
import { localizedError, TRANSLATED_MESSAGES } from "./errors";
import { Glob } from "bun";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

async function backendErrorMessages(root: URL) {
  const messages: Array<{ file: string; message: string }> = [];
  let files = 0;
  // internal(error) only logs its argument; its public message is covered by
  // Self::error inside ApiResponse::internal. Include that call and format!.
  const literal = /(?:ApiResponse|Self)::(?:error\(\s*[\w:]+\s*,\s*|(?:bad_request|unauthorized|forbidden|not_found|conflict|unprocessable|throttled)\(\s*)(format!\(\s*)?"((?:\\.|[^"\\])*)"/g;
  for await (const file of new Glob('**/*.rs').scan({ cwd: fileURLToPath(root), absolute: true })) {
    files++;
    const source = await readFile(file, 'utf8');
    for (const match of source.matchAll(literal)) {
      const message = match[1] ? match[2].replace(/\{[^{}]*\}/g, '42') : match[2];
      if (/[\u4e00-\u9fff]/.test(message)) messages.push({ file, message });
    }
  }
  // A broken cwd or extraction pattern must never turn this guard into a pass.
  expect(files, `No Rust sources scanned at ${root}`).toBeGreaterThan(0);
  expect(messages.length, `No API messages found at ${root}`).toBeGreaterThan(0);
  return messages;
}

test('backend API error literals and format templates have English translations', async () => {
  for (const { file, message } of await backendErrorMessages(new URL('../../backend/src/', import.meta.url))) {
    expect(localizedError(message, 'en'), `${file}: ${message}`).not.toMatch(/[\u4e00-\u9fff]/);
  }
});

test('translation guard scans paths with spaces and Unicode and covers generic errors', async () => {
  const root = await mkdtemp(join(tmpdir(), 'nodeflare 中文 '));
  try {
    await writeFile(join(root, 'example.rs'), `
      ApiResponse::bad_request("节点不存在");
      ApiResponse::error(axum::http::StatusCode::BAD_REQUEST, "主题不存在");
      ApiResponse::throttled(format!("登录尝试过多，请在 {seconds} 秒后重试"), seconds);
      fn internal(error: impl Display) { Self::error(StatusCode::INTERNAL_SERVER_ERROR, "服务器内部错误") }
    `);
    expect((await backendErrorMessages(pathToFileURL(root))).map(value => value.message)).toEqual([
      '节点不存在', '主题不存在', '登录尝试过多，请在 42 秒后重试', '服务器内部错误',
    ]);
    await rm(join(root, 'example.rs'));
    await expect(backendErrorMessages(pathToFileURL(root))).rejects.toThrow();
  } finally { await rm(root, { recursive: true, force: true }); }
});

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
