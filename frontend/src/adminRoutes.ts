export type AdminTab = "servers" | "latency" | "appearance" | "themes" | "themeSettings" | "alerts" | "security" | "data" | "remote" | "about";

export const adminTabPaths: Record<AdminTab, string> = {
  servers: "/admin/servers",
  latency: "/admin/latency",
  remote: "/admin/remote",
  alerts: "/admin/alerts",
  themes: "/admin/themes",
  themeSettings: "/admin/theme-settings",
  appearance: "/admin/site-settings",
  security: "/admin/security",
  data: "/admin/database",
  about: "/admin/about",
};

export function adminTabFromPath(pathname: string): AdminTab {
  const normalized = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  return (Object.entries(adminTabPaths).find(([, path]) => path === normalized)?.[0] as AdminTab | undefined) ?? "servers";
}
