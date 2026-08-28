import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { api } from "./api";
import { AdminPanel } from "./components/AdminPanel";
import type { Config } from "./types";
import "./styles/admin.css";

const THEME_KEY = "nodeflare-admin-theme";

function initialAppearance(): "light" | "dark" | null {
  const stored = localStorage.getItem(THEME_KEY);
  return stored === "light" || stored === "dark" ? stored : null;
}

function AdminApp() {
  const [config, setConfig] = useState<Config | null>(null);
  const [error, setError] = useState("");
  const [appearance, setAppearance] = useState<"light" | "dark" | null>(initialAppearance);
  const [systemDark, setSystemDark] = useState(() => matchMedia("(prefers-color-scheme: dark)").matches);
  const dark = appearance ? appearance === "dark" : config?.default_theme === "system" || !config ? systemDark : config.default_theme === "dark";

  async function loadConfig() {
    try {
      const next = await api.config();
      setConfig(next);
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
    const media = matchMedia("(prefers-color-scheme: dark)");
    const update = () => setSystemDark(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  useEffect(() => {
    document.title = config ? `管理面板 · ${config.site_name}` : "管理面板";
  }, [config]);
  useEffect(() => {
    let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!link) {
      link = document.createElement("link");
      link.rel = "icon";
      document.head.append(link);
    }
    link.removeAttribute("type");
    link.href = config?.logo_url || "/logo.svg";
  }, [config?.logo_url]);

  if (error) return <div className={`admin-loading ${dark ? "admin-dark" : ""}`}><span>{error}</span><button className="secondary-btn" onClick={() => void loadConfig()}>重试</button></div>;
  if (!config) return <div className={`admin-loading ${dark ? "admin-dark" : ""}`}>正在加载管理面板</div>;
  return <AdminPanel config={config} dark={dark} onToggleTheme={() => {
    const next = dark ? "light" : "dark";
    localStorage.setItem(THEME_KEY, next);
    setAppearance(next);
  }} onClose={() => window.location.assign("/")} onChanged={() => void loadConfig()} />;
}

createRoot(document.getElementById("root")!).render(<StrictMode><AdminApp /></StrictMode>);
