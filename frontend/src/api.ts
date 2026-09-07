import type { AdminServer, AlertRule, AlertRuleInput, Bootstrap, Config, DatabaseMigrationResult, DatabaseStats, ExchangeRates, HistoryPoint, LatencySample, LatencyTask, LatencyTaskInput, LatencyTestPoint, LoginSession, RemoteTask, RemoteTaskCreated, RemoteTaskInput, ServerInput, Settings, TelegramSettings, TelegramSettingsInput, Theme, ThemeSettingsSchema, TotpSetup, TotpStatus } from "./types";

export const ADMIN_UNAUTHORIZED_EVENT = "nodeflare:admin-unauthorized";

export class ApiError extends Error {
  constructor(message: string, public status: number) {
    super(message);
  }
}

async function requestResponse(path: string, init: RequestInit = {}, admin = false): Promise<Response> {
  const headers = new Headers(init.headers);
  if (typeof init.body === "string" && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  const response = await fetch(path, { ...init, headers, credentials: "same-origin" });
  if (!response.ok) {
    const payload = await response.json().catch(() => ({ error: response.statusText }));
    if (response.status === 401 && admin) {
      window.dispatchEvent(new Event(ADMIN_UNAUTHORIZED_EVENT));
      throw new ApiError("", response.status);
    }
    throw new ApiError(payload.error ?? "请求失败", response.status);
  }
  return response;
}

async function request<T>(path: string, init: RequestInit = {}, admin = false): Promise<T> {
  const response = await requestResponse(path, init, admin);
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

interface SensitiveProof {
  totpCode?: string;
  passwordDerived?: string;
}

function sensitiveHeaders(proof: SensitiveProof): Headers {
  const headers = new Headers();
  if (proof.totpCode) headers.set("X-NodeFlare-TOTP", proof.totpCode);
  if (proof.passwordDerived) headers.set("X-NodeFlare-Password", proof.passwordDerived);
  return headers;
}

export const api = {
  bootstrap: () => request<Bootstrap>("/api/bootstrap", { cache: "no-store", signal: AbortSignal.timeout(15_000) }),
  config: () => request<Config>("/api/config"),
  exchangeRates: () => request<ExchangeRates>("/api/exchange-rates"),
  wakeServers: (serverIds: string[]) => request<void>("/api/live/wake", {
    method: "POST",
    body: JSON.stringify({ server_ids: serverIds }),
    signal: AbortSignal.timeout(15_000),
  }),
  refreshExchangeRates: () => request<ExchangeRates>("/api/admin/exchange-rates/refresh", { method: "POST" }, true),
  adminServers: () => request<{ servers: AdminServer[] }>("/api/admin/servers", {}, true),
  history: (id: string, hours: number) =>
    request<{ points: HistoryPoint[] }>(`/api/history/${encodeURIComponent(id)}?hours=${hours}`, { cache: "no-store", signal: AbortSignal.timeout(15_000) }),
  latencyHistory: (id: string, hours: number) =>
    request<{ tasks: LatencyTestPoint[]; points: LatencySample[] }>(`/api/latency/${encodeURIComponent(id)}?hours=${hours}`, { cache: "no-store", signal: AbortSignal.timeout(15_000) }),
  verifyTurnstile: (token: string) =>
    request<void>("/api/turnstile/verify", { method: "POST", body: JSON.stringify({ token }) }),
  login: async (username: string, passwordDerived: string, turnstileToken: string, totpCode = "") => {
    await request<{ token: string }>("/api/admin/login", {
      method: "POST",
      body: JSON.stringify({ username, password_derived: passwordDerived, turnstile_token: turnstileToken, totp_code: totpCode }),
    });
  },
  logout: () => request<void>("/api/admin/logout", { method: "POST" }),
  loginSessions: () => request<{ sessions: LoginSession[] }>("/api/admin/sessions", {}, true),
  revokeLoginSession: (id: string) =>
    request<void>(`/api/admin/sessions/${encodeURIComponent(id)}`, { method: "DELETE" }, true),
  settings: () => request<Settings>("/api/admin/settings", {}, true),
  twoFactorStatus: () => request<TotpStatus>("/api/admin/2fa/status", {}, true),
  setupTwoFactor: (proof: SensitiveProof) => request<TotpSetup>("/api/admin/2fa/setup", {
    method: "POST",
    headers: sensitiveHeaders(proof),
  }, true),
  enableTwoFactor: (totpCode: string) => request<void>("/api/admin/2fa/enable", {
    method: "POST",
    body: JSON.stringify({ totp_code: totpCode }),
  }, true),
  disableTwoFactor: (totpCode: string) => request<void>("/api/admin/2fa/disable", {
    method: "POST",
    body: JSON.stringify({ totp_code: totpCode }),
  }, true),
  latencyTasks: () => request<{ tasks: LatencyTask[] }>("/api/admin/latency-tasks", {}, true),
  createLatencyTask: (input: LatencyTaskInput) =>
    request<{ id: string }>("/api/admin/latency-tasks", { method: "POST", body: JSON.stringify(input) }, true),
  updateLatencyTask: (id: string, input: LatencyTaskInput) =>
    request<void>(`/api/admin/latency-tasks/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(input) }, true),
  deleteLatencyTask: (id: string) =>
    request<void>(`/api/admin/latency-tasks/${encodeURIComponent(id)}`, { method: "DELETE" }, true),
  alertRules: () => request<{ rules: AlertRule[] }>("/api/admin/alert-rules", {}, true),
  createAlertRule: (input: AlertRuleInput) =>
    request<{ id: string }>("/api/admin/alert-rules", { method: "POST", body: JSON.stringify(input) }, true),
  updateAlertRule: (id: string, input: AlertRuleInput) =>
    request<void>(`/api/admin/alert-rules/${encodeURIComponent(id)}`, { method: "PATCH", body: JSON.stringify(input) }, true),
  deleteAlertRule: (id: string) =>
    request<void>(`/api/admin/alert-rules/${encodeURIComponent(id)}`, { method: "DELETE" }, true),
  telegramSettings: () => request<{ telegram: TelegramSettings | null }>("/api/admin/telegram", {}, true),
  saveTelegramSettings: (input: TelegramSettingsInput) =>
    request<void>("/api/admin/telegram", { method: "PUT", body: JSON.stringify(input) }, true),
  testTelegram: () => request<void>("/api/admin/telegram/test", { method: "POST" }, true),
  themeSettings: () => request<ThemeSettingsSchema>("/api/admin/theme-settings", {}, true),
  themes: () => request<{ themes: Theme[] }>("/api/admin/themes", {}, true),
  addTheme: (input: Pick<Theme, "name" | "description" | "url">) =>
    request<{ id: string }>("/api/admin/themes", { method: "POST", body: JSON.stringify(input) }, true),
  uploadTheme: (input: Pick<Theme, "name" | "description">, file: File) => {
    const query = new URLSearchParams({ ...input, filename: file.name });
    return request<{ id: string }>(`/api/admin/themes/upload?${query}`, { method: "POST", body: file }, true);
  },
  activateTheme: (id: string) =>
    request<void>(`/api/admin/themes/${encodeURIComponent(id)}/activate`, { method: "POST" }, true),
  previewTheme: (id: string) =>
    request<{ preview_url: string }>(`/api/admin/themes/${encodeURIComponent(id)}/preview`, { method: "POST" }, true),
  deleteTheme: (id: string) =>
    request<void>(`/api/admin/themes/${encodeURIComponent(id)}`, { method: "DELETE" }, true),
  saveSettings: (input: Partial<Settings>) =>
    request<Settings>("/api/admin/settings", { method: "PATCH", body: JSON.stringify(input) }, true),
  createServer: (input: ServerInput) =>
    request<{ id: string; agent_token: string }>(
      "/api/admin/servers",
      { method: "POST", body: JSON.stringify(input) },
      true,
    ),
  createAgentInstallToken: (id: string) =>
    request<{ agent_token: string }>(`/api/admin/servers/${encodeURIComponent(id)}/agent-token`, { method: "POST" }, true),
  updateServer: (id: string, input: ServerInput) =>
    request<void>(
      `/api/admin/servers/${encodeURIComponent(id)}`,
      { method: "PATCH", body: JSON.stringify(input) },
      true,
    ),
  deleteServer: (id: string) =>
    request<void>(
      `/api/admin/servers/${encodeURIComponent(id)}`,
      { method: "DELETE" },
      true,
    ),
  deleteServers: (ids: string[]) =>
    request<void>("/api/admin/servers", {
      method: "DELETE",
      body: JSON.stringify({ ids }),
    }, true),
  reorderServers: (ids: string[]) =>
    request<void>(
      "/api/admin/servers/order",
      { method: "PATCH", body: JSON.stringify({ ids }) },
      true,
    ),
  databaseStats: () => request<DatabaseStats>("/api/admin/database", {}, true),
  reclaimDatabase: () => request<{ database: DatabaseStats; reclaimed_bytes: number }>(
    "/api/admin/database/reclaim",
    { method: "POST" },
    true,
  ),
  migrateDatabase: (databaseUrl: string, proof: SensitiveProof) => request<DatabaseMigrationResult>(
    "/api/admin/database/migrate",
    { method: "POST", headers: sensitiveHeaders(proof), body: JSON.stringify({ database_url: databaseUrl }) },
    true,
  ),
  restartAfterDatabaseMigration: () => request<{ restarting: boolean }>(
    "/api/admin/database/restart",
    { method: "POST" },
    true,
  ),
  databaseBackup: async (proof: SensitiveProof) => {
    const response = await requestResponse("/api/admin/database/backup", {
      headers: sensitiveHeaders(proof),
    }, true);
    const disposition = response.headers.get("Content-Disposition") ?? "";
    const filename = /filename="?([^";]+)"?/i.exec(disposition)?.[1] ?? "nodeflare-database-backup.zip";
    return { blob: await response.blob(), filename };
  },
  restoreDatabaseBackup: (
    file: File,
    proof: SensitiveProof,
  ) => {
    const query = new URLSearchParams({ filename: file.name });
    return request<{ restored_rows: number }>(`/api/admin/database/restore?${query}`, {
      method: "POST",
      body: file,
      headers: sensitiveHeaders(proof),
    }, true);
  },
  createRemoteTask: (input: RemoteTaskInput) => request<{ tasks: RemoteTaskCreated[] }>("/api/admin/remote/task", {
    method: "POST",
    body: JSON.stringify(input),
  }, true),
  remoteTask: (id: string) => request<RemoteTask>(`/api/admin/remote/task/${encodeURIComponent(id)}`, {}, true),
};
