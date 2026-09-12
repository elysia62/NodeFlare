import { CircleAlert, KeyRound } from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";
import { ApiError } from "../api";
import { ui } from "../locale";
import type { Config } from "../types";
import { PasswordInput } from "./PasswordInput";
import { TurnstileWidget } from "./TurnstileWidget";

// Shared sign-in form for the public dashboard gate and the admin panel.
// Owns the credential state machine: busy guard against duplicate submits,
// 428 TOTP challenge escalation, Turnstile reset on failure. The submit
// callback receives the raw credentials; password derivation stays with the
// caller. Errors are controlled by the caller so unrelated load failures can
// surface in the same place.
export function LoginForm({ config, dark, error, setError, onSubmit, className, turnstileClassName, errorClassName, submitClassName, children }: {
  config: Config;
  dark: boolean;
  error: string;
  setError: (message: string) => void;
  onSubmit: (username: string, password: string, turnstileToken: string, totpCode: string) => Promise<void>;
  className: string;
  turnstileClassName: string;
  errorClassName: string;
  submitClassName: string;
  children?: ReactNode;
}) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [turnstileToken, setTurnstileToken] = useState("");
  const [turnstileReset, setTurnstileReset] = useState(0);
  const [totpCode, setTotpCode] = useState("");
  const [totpChallenge, setTotpChallenge] = useState(false);
  const [busy, setBusy] = useState(false);
  const locale = config.locale;
  const totpRequired = config.totp_login_enabled || totpChallenge;
  const describedBy = error ? "login-form-error" : undefined;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError("");
    try {
      await onSubmit(username, password, turnstileToken, totpCode);
      setPassword("");
      setTurnstileToken("");
      setTotpCode("");
      setTotpChallenge(false);
    } catch (reason) {
      if (reason instanceof ApiError && reason.status === 428) setTotpChallenge(true);
      setError(reason instanceof Error ? reason.message : ui(locale, "登录失败", "Unable to sign in"));
      setTurnstileToken("");
      setTurnstileReset((value) => value + 1);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className={className} onSubmit={(event) => void submit(event)}>
      {children}
      <label><span>{ui(locale, "用户名", "Username")}</span><input autoFocus autoComplete="username" value={username} onChange={(event) => setUsername(event.target.value)} required aria-invalid={error ? true : undefined} aria-describedby={describedBy} /></label>
      <label><span>{ui(locale, "密码", "Password")}</span><PasswordInput locale={locale} autoComplete="current-password" value={password} onChange={(event) => setPassword(event.target.value)} required aria-invalid={error ? true : undefined} aria-describedby={describedBy} /></label>
      {totpRequired ? <label><span>{ui(locale, "两步验证码", "Two-factor code")}</span><input autoFocus inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" maxLength={6} value={totpCode} onChange={(event) => setTotpCode(event.target.value.replace(/\D/g, "").slice(0, 6))} required aria-invalid={error ? true : undefined} aria-describedby={describedBy} /></label> : null}
      {config.turnstile_login_enabled ? <div className={turnstileClassName}><TurnstileWidget siteKey={config.turnstile_site_key} action="admin_login" theme={dark ? "dark" : "light"} resetKey={turnstileReset} onVerify={setTurnstileToken} onError={setError} locale={locale} /></div> : null}
      {error ? <p className={errorClassName} id="login-form-error" role="alert"><CircleAlert size={15} aria-hidden="true" />{error}</p> : null}
      <button className={submitClassName} disabled={busy || (totpRequired && !/^\d{6}$/.test(totpCode)) || (config.turnstile_login_enabled && !turnstileToken)} type="submit"><KeyRound size={15} />{busy ? ui(locale, "登录中", "Signing in") : ui(locale, "登录", "Sign in")}</button>
    </form>
  );
}
