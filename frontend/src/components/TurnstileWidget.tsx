import { useEffect, useRef, useState } from "react";

type TurnstileApi = {
  render: (target: HTMLElement, options: Record<string, unknown>) => string;
  remove: (widgetId: string) => void;
  reset: (widgetId: string) => void;
};

declare global {
  interface Window {
    turnstile?: TurnstileApi;
  }
}

let scriptPromise: Promise<void> | null = null;

function loadScript() {
  if (window.turnstile) return Promise.resolve();
  if (scriptPromise) return scriptPromise;
  scriptPromise = new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = "https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit";
    script.async = true;
    script.defer = true;
    script.onload = () => resolve();
    script.onerror = () => {
      scriptPromise = null;
      reject(new Error("Cloudflare Turnstile 加载失败"));
    };
    document.head.appendChild(script);
  });
  return scriptPromise;
}

export function TurnstileWidget({
  siteKey,
  action,
  theme,
  resetKey,
  onVerify,
  onError,
}: {
  siteKey: string;
  action: "admin_login" | "public_dashboard";
  theme: "light" | "dark";
  resetKey?: number;
  onVerify: (token: string) => void;
  onError: (message: string) => void;
}) {
  const target = useRef<HTMLDivElement>(null);
  const widgetId = useRef("");
  const [verified, setVerified] = useState(false);
  const verifyCallback = useRef(onVerify);
  const errorCallback = useRef(onError);
  verifyCallback.current = onVerify;
  errorCallback.current = onError;

  useEffect(() => {
    let cancelled = false;
    setVerified(false);
    verifyCallback.current("");
    void loadScript().then(() => {
      if (cancelled || !target.current || !window.turnstile) return;
      target.current.replaceChildren();
      widgetId.current = window.turnstile.render(target.current, {
        sitekey: siteKey,
        action,
        theme,
        size: "flexible",
        callback: (token: string) => {
          setVerified(true);
          verifyCallback.current(token);
        },
        "expired-callback": () => {
          setVerified(false);
          verifyCallback.current("");
        },
        "error-callback": () => {
          setVerified(false);
          verifyCallback.current("");
          errorCallback.current("Cloudflare 验证暂时不可用，请刷新重试");
        },
      });
    }).catch((reason) => errorCallback.current(reason instanceof Error ? reason.message : "验证组件加载失败"));
    return () => {
      cancelled = true;
      if (widgetId.current && window.turnstile) window.turnstile.remove(widgetId.current);
      widgetId.current = "";
    };
  }, [action, siteKey, theme, resetKey]);

  const testing = siteKey.startsWith("1x00000000000000000000");
  return <div className={`turnstile-widget${verified ? " verified" : ""}${testing ? " testing" : ""}`} ref={target} />;
}
