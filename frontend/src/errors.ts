import type { UiLocale } from "./locale";

/**
 * English text for the messages the backend returns.
 *
 * The API answers with Simplified Chinese, which is correct for the default
 * deployment but leaves an English dashboard showing a mix of both languages.
 * Rather than duplicating the strings in the Rust handlers, the messages are
 * translated here at the single point where an error becomes user-visible
 * (`ApiError`), so every caller keeps reading `error.message` unchanged.
 *
 * Keys are the exact server strings. Messages that interpolate a runtime value
 * use `{placeholders}` and are matched by pattern.
 */
const MESSAGES: Record<string, string> = {
  "Agent Token 无效": "Invalid Agent token",
  "Cloudflare 人机验证失败，请重试": "Cloudflare verification failed; please try again",
  "ID 无效": "Invalid ID",
  "Telegram 配置格式无效": "Invalid Telegram configuration",
  "Turnstile Site Key 和 Secret Key 必须同时填写或同时留空":
    "Turnstile Site Key and Secret Key must both be set or both be empty",
  "上传文件必须是名称有效的 ZIP 文件": "The uploaded file must have a valid ZIP filename",
  "两步验证码错误": "Incorrect two-factor code",
  "任务 ID 无效": "Invalid task ID",
  "任务不存在": "Task not found",
  "主题 ZIP 不能超过 32 MiB": "The theme ZIP cannot exceed 32 MiB",
  "主题不存在": "Theme not found",
  "主题名称或说明无效": "Invalid theme name or description",
  "主题设置格式无效": "Invalid theme settings",
  "使用 --database 启动时无法自动切换配置":
    "The configuration cannot be switched automatically when started with --database",
  "公开仪表盘未启用人机验证": "Human verification is not enabled for the public dashboard",
  "内置主题不能删除": "The built-in theme cannot be deleted",
  "告警规则不存在": "Alert rule not found",
  "告警规则包含不存在的节点": "The alert rule references a server that does not exist",
  "告警规则格式无效": "Invalid alert rule",
  "密码验证繁忙，请稍后重试": "Password verification is busy; please try again",
  "尚未配置两步验证": "Two-factor authentication is not configured",
  "延迟任务不存在": "Latency task not found",
  "延迟任务包含不存在的节点": "The latency task references a server that does not exist",
  "延迟任务格式无效": "Invalid latency task",
  "当前主题不存在": "The current theme does not exist",
  "当前密码错误": "Incorrect current password",
  "当前没有等待生效的数据库迁移": "No database migration is pending",
  "所选节点均未连接，命令未下发":
    "None of the selected servers are connected; the command was not sent",
  "排序列表必须包含全部节点": "The order list must contain every server",
  "接口不存在": "Endpoint not found",
  "数据库备份 ZIP 不能超过 512 MiB": "The database backup ZIP cannot exceed 512 MiB",
  "数据库正在维护，请稍后重试": "The database is under maintenance; please try again",
  "数据库迁移已完成，请重启服务后继续修改":
    "The database migration is complete; restart the service to continue",
  "新密码摘要格式无效": "Invalid new-password digest",
  "无法初始化目标数据库，请检查账号权限":
    "Cannot initialize the target database; check the account permissions",
  "无法连接目标数据库，请检查地址、账号和网络":
    "Cannot connect to the target database; check the address, account and network",
  "最多可创建 20 条资源告警规则": "At most 20 resource alert rules can be created",
  "此仪表盘需要登录后访问": "This dashboard requires signing in",
  "活动主题不存在": "The active theme does not exist",
  "用户名或密码错误": "Incorrect username or password",
  "登录状态已过期": "The session has expired",
  "登录设备 ID 无效": "Invalid login device ID",
  "登录设备不存在": "Login device not found",
  "登录验证繁忙，请稍后重试": "Login verification is busy; please try again",
  "目标必须使用另一种数据库类型": "The target must use the other database type",
  "目标数据库 URL 格式无效": "Invalid target database URL",
  "站点设置格式无效": "Invalid site settings",
  "管理员用户名格式无效": "Invalid administrator username",
  "节点 ID 无效": "Invalid server ID",
  "节点不存在": "Server not found",
  "节点列表无效": "Invalid server list",
  "节点选择列表无效": "Invalid server selection",
  "该主题 ZIP 已添加": "That theme ZIP has already been added",
  "该主题已添加": "That theme has already been added",
  "请先使用当前验证码禁用两步验证，再重新生成密钥":
    "Disable two-factor authentication with a current code before generating a new secret",
  "请先在登录与安全中启用 TOTP 两步验证":
    "Enable TOTP two-factor authentication under Login & Security first",
  "请先完成人机验证": "Complete human verification first",
  "请先生成两步验证密钥": "Generate a two-factor secret first",
  "请先登录": "Please sign in",
  "请求主机名无效": "Invalid request hostname",
  "请输入两步验证码": "Enter your two-factor code",
  "请输入有效的目标数据库 URL": "Enter a valid target database URL",
  "请输入用户名和密码": "Enter your username and password",
  "请选择 1 至 128 个节点并填写命令":
    "Select 1 to 128 servers and enter a command",
  "请选择有效的数据库备份 ZIP": "Select a valid database backup ZIP",
  "请选择非空 ZIP 文件": "Select a non-empty ZIP file",
};

/**
 * Messages that embed a runtime value. Each entry pairs a matcher over the
 * server string with a formatter for the English text.
 */
const TEMPLATES: Array<{
  match: RegExp;
  render: (groups: string[]) => string;
}> = [
  {
    match: /^Agent 协议不兼容，需要协议版本 (.+)$/,
    render: (g) => `Incompatible Agent protocol; version ${g[0]} is required`,
  },
  {
    match: /^人机验证尝试过多，请在 (\d+) 秒后重试$/,
    render: (g) => `Too many verification attempts; retry in ${g[0]}s`,
  },
  {
    match: /^恢复失败：(.+)$/,
    render: (g) => `Restore failed: ${g[0]}`,
  },
  {
    match: /^登录尝试过多，请在 (\d+) 秒后重试$/,
    render: (g) => `Too many login attempts; retry in ${g[0]}s`,
  },
  {
    match:
      /^目标数据库已写入数据，但配置更新失败（(.+)）；请手动修改 database_url 后重启面板$/,
    render: (g) =>
      `The target database has been written, but updating the configuration failed (${g[0]}). ` +
      "Edit database_url manually and restart the panel",
  },
  {
    match: /^节点不存在: (.+)$/,
    render: (g) => `Server not found: ${g[0]}`,
  },
  {
    match: /^请求过于频繁，请在 (\d+) 秒后重试$/,
    render: (g) => `Too many requests; retry in ${g[0]}s`,
  },
  {
    match: /^迁移失败：(.+)$/,
    render: (g) => `Migration failed: ${g[0]}`,
  },
  {
    match: /^验证尝试过多，请在 (\d+) 秒后重试$/,
    render: (g) => `Too many verification attempts; retry in ${g[0]}s`,
  },
  {
    match: /^验证码尝试过多，请在 (\d+) 秒后重试$/,
    render: (g) => `Too many code attempts; retry in ${g[0]}s`,
  },
];

/**
 * Returns the message to display for a backend error.
 *
 * Unknown messages are returned unchanged, so a new server-side string shows
 * its original text instead of disappearing.
 */
export function localizedError(message: string, locale: UiLocale | string | undefined): string {
  if (locale !== "en" || !message) return message;
  const exact = MESSAGES[message];
  if (exact) return exact;
  for (const template of TEMPLATES) {
    const groups = template.match.exec(message);
    if (groups) return template.render(groups.slice(1));
  }
  return message;
}

/** Exposed for tests: every literal key that should have a translation. */
export const TRANSLATED_MESSAGES = MESSAGES;
