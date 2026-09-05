import { ShieldCheck, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { PasswordInput } from "./PasswordInput";
import { useDialog } from "./useDialog";

export function useVerification(active: boolean) {
  const [request, setRequest] = useState<{ title: string; totp: boolean } | null>(null);
  const [value, setValue] = useState("");
  const resolver = useRef<((value: string | null) => void) | null>(null);
  const finish = useCallback((result: string | null) => {
    resolver.current?.(result);
    resolver.current = null;
    setRequest(null);
    setValue("");
  }, []);
  const dialog = useDialog<HTMLFormElement>(request !== null, () => finish(null));
  useEffect(() => () => { resolver.current?.(null); }, []);
  useEffect(() => { if (!active) finish(null); }, [active, finish]);
  const ask = useCallback((title: string, totp: boolean) => new Promise<string | null>((resolve) => {
    resolver.current?.(null);
    resolver.current = resolve;
    setValue("");
    setRequest({ title, totp });
  }), []);

  return {
    ask,
    dialog: request ? <div className="submodal-backdrop verification-backdrop" onMouseDown={dialog.onBackdropMouseDown}>
      <form ref={dialog.dialogRef} className="editor-modal verification-modal" role="dialog" aria-modal="true" aria-labelledby="verification-title" tabIndex={-1} onSubmit={(event) => { event.preventDefault(); finish(value); }}>
        <header><h3 id="verification-title">{request.title}</h3><button type="button" className="icon-btn" title="关闭" aria-label="关闭" onClick={() => finish(null)}><X size={18} /></button></header>
        <label><span>{request.totp ? "两步验证码" : "当前管理员密码"}</span>{request.totp
          ? <input autoFocus data-dialog-autofocus required inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" maxLength={6} value={value} onChange={(event) => setValue(event.target.value.replace(/\D/g, "").slice(0, 6))} />
          : <PasswordInput autoFocus data-dialog-autofocus required autoComplete="current-password" maxLength={128} value={value} onChange={(event) => setValue(event.target.value)} />}</label>
        <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => finish(null)}>取消</button><button className="primary-btn" disabled={request.totp ? !/^\d{6}$/.test(value) : !value}><ShieldCheck size={15} />确认</button></div>
      </form>
    </div> : null,
  };
}
