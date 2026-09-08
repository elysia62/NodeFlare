import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { api } from "./api";
import { AdminPanel } from "./components/AdminPanel";
import { useFavicon, useStoredAppearance, useSystemDark } from "./hooks/useBrowserAppearance";
import type { Config } from "./types";
import "./styles/admin.css";

function AdminApp() {
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState("");
  const [appearance, setAppearance] = useStoredAppearance("nodeflare-admin-theme");
  const systemDark = useSystemDark();
  const dark = appearance ? appearance === "dark" : config?.default_theme === "system" || !config ? systemDark : config.default_theme === "dark";
  useFavicon(config?.logo_url);

  async function loadConfig() {
    try {
      const next = await api.bootstrap();
      setConfig(next.config);
      setError("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : "无法加载管理面板");
    }
  }

  useEffect(() => { void loadConfig(); }, []);
  useEffect(() => {
    document.documentElement.classList.toggle("dark", dark);
    document.documentElement.style.colorScheme = dark ? "dark" : "light";
  }, [dark]);
  useEffect(() => {
    document.title = config ? `管理面板 · ${config.site_name}` : "管理面板";
  }, [config]);

  if (error) return <div className={`admin-loading ${dark ? "admin-dark" : ""}`}><span>{error}</span><button className="secondary-btn" onClick={() => void loadConfig()}>重试</button></div>;
  if (!config) return <div className={`admin-loading ${dark ? "admin-dark" : ""}`}>正在加载管理面板</div>;
  return <AdminPanel config={config} dark={dark} onToggleTheme={() => {
    const next = dark ? "light" : "dark";
    setAppearance(next);
  }} onChanged={() => void loadConfig()} />;
}

createRoot(document.getElementById("root")!).render(<StrictMode><AdminApp /></StrictMode>);
