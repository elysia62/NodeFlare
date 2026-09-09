import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, ApiError } from "./api";
import { demoConfig, demoExchangeRates, demoServers } from "./demo";
import {
  applyBatch,
  mergeServerLive,
  pruneLiveMetrics,
  type BatchUpdate,
  type LiveMetricsMap,
} from "./live";
import { ui } from "./locale";
import { useFavicon, useStoredAppearance, useSystemDark } from "./hooks/useBrowserAppearance";
import { resolveBackground, themeToggle } from "./theme";
import {
  BOOTSTRAP_POLL_INTERVAL_MS,
  createRefreshQueue,
  LIVE_CARD_REFRESH_INTERVAL_MS,
  shouldSyncBootstrap,
} from "./refresh";
import { connectLive } from "./transport";
import type { Config, ExchangeRates, Server } from "./types";
import { derivePassword } from "./password";

const search = new URLSearchParams(window.location.search);
export const demoMode = import.meta.env.DEV && search.has("demo");
const demoViewConfig: Config = demoMode && search.has("carrier")
  ? { ...demoConfig, theme_options: { ...demoConfig.theme_options, showCarrierLatency: true } }
  : demoConfig;
const defaultConfig: Config = { ...demoViewConfig, site_description: "", site_name: "" };

type Access = "ok" | "login" | "turnstile";

export interface AppState {
  config: Config;
  configReady: boolean;
  servers: Server[];
  liveMetrics: LiveMetricsMap;
  liveConnected: boolean;
  exchangeRates: ExchangeRates | null;
  loading: boolean;
  error: string;
  setError: (message: string) => void;
  access: Access;
  dark: boolean;
  blur: boolean;
  background: string;
  carrierLatency: boolean;
  toggleTheme: () => void;
  selectedId: string | null;
  openServer: (server: Server) => void;
  goHome: () => void;
  reload: () => Promise<void>;
  login: (username: string, password: string, turnstileToken: string, totpCode: string) => Promise<void>;
  verify: (token: string) => Promise<void>;
}

const AppContext = createContext<AppState | null>(null);

export function useApp(): AppState {
  const state = useContext(AppContext);
  if (!state) throw new Error("useApp must be used within AppProvider");
  return state;
}

function routeServerId() {
  const match = window.location.pathname.match(/^\/instance\/([^/]+)\/?$/);
  if (!match) return null;
  try { return decodeURIComponent(match[1]); } catch { return null; }
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [config, setConfig] = useState(defaultConfig);
  const [configReady, setConfigReady] = useState(demoMode);
  const [servers, setServers] = useState<Server[]>([]);
  const [liveMetrics, setLiveMetrics] = useState<LiveMetricsMap>({});
  const [liveConnected, setLiveConnected] = useState(false);
  const pendingLive = useRef<BatchUpdate[]>([]);
  const [clockNow, setClockNow] = useState(() => Date.now());
  const [exchangeRates, setExchangeRates] = useState<ExchangeRates | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [access, setAccess] = useState<Access>("ok");
  const [selectedId, setSelectedId] = useState<string | null>(routeServerId);
  const [appearance, setAppearance] = useStoredAppearance("nodeflare-theme");
  const systemDark = useSystemDark();
  const liveConnectedRef = useRef(false);
  const serversRef = useRef<Server[]>([]);
  const localeRef = useRef(config.locale);
  const reloadQueueRef = useRef<ReturnType<typeof createRefreshQueue> | null>(null);

  localeRef.current = config.locale;

  const dark = appearance ? appearance === "dark" : config.default_theme === "system" ? systemDark : config.default_theme === "dark";
  const background = resolveBackground(config.background_url, dark);
  const blur = themeToggle(config, "enableBlur");
  const carrierLatency = config.show_latency && themeToggle(config, "showCarrierLatency", false);
  useFavicon(config.logo_url);

  if (!reloadQueueRef.current) {
    reloadQueueRef.current = createRefreshQueue(async (quiet) => {
      if (!quiet) setLoading(true);
      if (demoMode) {
        setConfig(demoViewConfig);
        setConfigReady(true);
        setServers(demoServers);
        setExchangeRates(demoExchangeRates);
        setError("");
        setLoading(false);
        return;
      }
      try {
        const result = await api.bootstrap();
        setConfig(result.config);
        setConfigReady(true);
        serversRef.current = result.servers;
        setServers(result.servers);
        setExchangeRates(result.exchange_rates);
        setAccess(result.access);
        if (result.access !== "ok") {
          setLiveMetrics({});
        }
        setError("");
      } catch (reason) {
        const status = reason instanceof ApiError ? reason.status : 0;
        if (status === 401 || status === 403) {
          setAccess(status === 401 ? "login" : "turnstile");
          serversRef.current = [];
          setServers([]);
          setLiveMetrics({});
          setError(status === 403 && reason instanceof Error ? reason.message : "");
        } else {
          setError(reason instanceof Error ? reason.message : ui(localeRef.current, "无法加载节点状态", "Unable to load server status"));
        }
      } finally {
        setLoading(false);
      }
    });
  }

  const reload = useCallback((quiet = false): Promise<void> => {
    return reloadQueueRef.current!(quiet);
  }, []);

  const login = useCallback(async (username: string, password: string, turnstileToken: string, totpCode: string) => {
    const derived = await derivePassword(password, config.password_client_salt);
    await api.login(username.trim(), derived, turnstileToken, totpCode);
    setAccess("ok");
    await reload();
  }, [config.password_client_salt, reload]);

  const verify = useCallback(async (token: string) => {
    await api.verifyTurnstile(token);
    setAccess("ok");
    await reload();
  }, [reload]);

  useEffect(() => { void reload(); }, [reload]);

  useEffect(() => {
    serversRef.current = servers;
    setLiveMetrics((current) => pruneLiveMetrics(current, servers));
  }, [servers]);

  useEffect(() => {
    if (access !== "ok" || demoMode) return;
    let stopped = false;
    let timer: number | undefined;
    let lastSyncAt = Date.now();

    const canRefresh = () => !document.hidden && navigator.onLine !== false;
    const clearTimer = () => {
      if (timer !== undefined) {
        window.clearTimeout(timer);
        timer = undefined;
      }
    };
    const schedule = () => {
      clearTimer();
      if (!stopped && canRefresh()) {
        timer = window.setTimeout(run, BOOTSTRAP_POLL_INTERVAL_MS);
      }
    };
    const run = async () => {
      clearTimer();
      if (stopped || !canRefresh()) return;
      const now = Date.now();
      if (shouldSyncBootstrap(liveConnectedRef.current, now - lastSyncAt)) {
        await reload(true);
        lastSyncAt = Date.now();
      }
      schedule();
    };
    const refreshNow = () => {
      clearTimer();
      if (stopped || !canRefresh()) return;
      void reload(true).finally(() => {
        if (stopped) return;
        lastSyncAt = Date.now();
        schedule();
      });
    };
    const handleVisibility = () => {
      if (document.hidden) clearTimer();
      else refreshNow();
    };
    const handleOffline = () => clearTimer();

    document.addEventListener("visibilitychange", handleVisibility);
    window.addEventListener("online", refreshNow);
    window.addEventListener("offline", handleOffline);
    schedule();
    return () => {
      stopped = true;
      clearTimer();
      document.removeEventListener("visibilitychange", handleVisibility);
      window.removeEventListener("online", refreshNow);
      window.removeEventListener("offline", handleOffline);
    };
  }, [access, reload]);

  useEffect(() => {
    let timer: number | undefined;
    const tick = () => {
      setClockNow(Date.now());
      if (!pendingLive.current.length) return;
      const batch = pendingLive.current;
      pendingLive.current = [];
      setLiveMetrics((current) => applyBatch(current, batch, serversRef.current));
    };
    const sync = () => {
      window.clearInterval(timer);
      timer = undefined;
      if (document.hidden || navigator.onLine === false) return;
      tick();
      timer = window.setInterval(tick, LIVE_CARD_REFRESH_INTERVAL_MS);
    };
    sync();
    document.addEventListener("visibilitychange", sync);
    window.addEventListener("online", sync);
    window.addEventListener("offline", sync);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", sync);
      window.removeEventListener("online", sync);
      window.removeEventListener("offline", sync);
    };
  }, []);

  useEffect(() => {
    if (!configReady || access !== "ok" || demoMode) return;
    let active = true;
    const disconnect = connectLive({ serverId: selectedId }, {
      onBatch: (updates) => {
        pendingLive.current.push(...updates);
      },
      onConnectedChange: (connected) => {
        liveConnectedRef.current = connected;
        setLiveConnected(connected);
        if (!connected) {
          pendingLive.current = [];
        }
        if (active && !connected && !document.hidden && navigator.onLine !== false) {
          void reload(true);
        }
      },
    });
    return () => {
      active = false;
      pendingLive.current = [];
      disconnect();
    };
  }, [access, configReady, reload, selectedId]);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    document.documentElement.style.colorScheme = dark ? "dark" : "light";
    document.documentElement.dataset.blur = blur ? "on" : "off";
    document.documentElement.lang = config.locale;
  }, [blur, config.locale, dark]);

  useEffect(() => {
    const onPop = () => setSelectedId(routeServerId());
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const metricServers = useMemo(
    () => servers.map((server) => mergeServerLive(server, liveMetrics[server.id], 0, config.offline_threshold_seconds)),
    [config.offline_threshold_seconds, liveMetrics, servers],
  );
  const liveServers = useMemo(
    () => metricServers.map((server) => mergeServerLive(server, undefined, clockNow, config.offline_threshold_seconds)),
    [clockNow, config.offline_threshold_seconds, metricServers],
  );

  useEffect(() => {
    if (!configReady) return;
    const selected = liveServers.find((server) => server.id === selectedId);
    document.title = selected ? `${selected.name} · ${config.site_name}` : config.site_name;
  }, [config.site_name, configReady, liveServers, selectedId]);

  const goHome = useCallback(() => {
    window.history.pushState({}, "", demoMode ? "/?demo=1" : "/");
    setSelectedId(null);
    window.scrollTo({ top: 0, behavior: "auto" });
  }, []);

  const openServer = useCallback((server: Server) => {
    window.history.pushState({}, "", `/instance/${encodeURIComponent(server.id)}${demoMode ? "?demo=1" : ""}`);
    setSelectedId(server.id);
    window.scrollTo({ top: 0, behavior: "auto" });
  }, []);

  const toggleTheme = useCallback(() => {
    setAppearance((current) => {
      const resolved = current ?? (dark ? "dark" : "light");
      const next = resolved === "dark" ? "light" : "dark";
      return next;
    });
  }, [dark]);

  const value = useMemo<AppState>(() => ({
    config,
    configReady,
    servers: liveServers,
    liveMetrics,
    liveConnected,
    exchangeRates,
    loading,
    error,
    setError,
    access,
    dark,
    blur,
    background,
    carrierLatency,
    toggleTheme,
    selectedId,
    openServer,
    goHome,
    reload: () => reload(),
    login,
    verify,
  }), [access, background, blur, carrierLatency, config, configReady, dark, error, exchangeRates, goHome, liveConnected, liveMetrics, liveServers, loading, login, openServer, reload, selectedId, toggleTheme, verify]);

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}
