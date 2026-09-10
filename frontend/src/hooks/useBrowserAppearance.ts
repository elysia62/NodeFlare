import { useEffect, useState } from "react";

type Appearance = "light" | "dark";

export function useStoredAppearance(key: string) {
  const [appearance, setAppearance] = useState<Appearance | null>(() => {
    const stored = localStorage.getItem(key);
    return stored === "light" || stored === "dark" ? stored : null;
  });

  useEffect(() => {
    if (appearance) localStorage.setItem(key, appearance);
  }, [appearance, key]);

  return [appearance, setAppearance] as const;
}

export function useSystemDark() {
  const [systemDark, setSystemDark] = useState(
    () => matchMedia("(prefers-color-scheme: dark)").matches,
  );

  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: dark)");
    const update = () => setSystemDark(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  return systemDark;
}

export function useFavicon(logoUrl: string | null | undefined) {
  useEffect(() => {
    // Keep the server-resolved icon while the site configuration is loading.
    if (logoUrl == null) return;
    let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
    if (!link) {
      link = document.createElement("link");
      link.rel = "icon";
      document.head.append(link);
    }
    link.removeAttribute("type");
    link.href = logoUrl.trim() || "/logo.svg";
  }, [logoUrl]);
}
