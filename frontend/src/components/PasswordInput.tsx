import { Eye, EyeOff } from "lucide-react";
import { useState, type InputHTMLAttributes } from "react";
import { ui, type UiLocale } from "../locale";

export function PasswordInput({ locale, ...props }: Omit<InputHTMLAttributes<HTMLInputElement>, "type"> & { locale?: UiLocale | string }) {
  const [visible, setVisible] = useState(false);
  const showLabel = ui(locale, "显示密码", "Show password");
  const hideLabel = ui(locale, "隐藏密码", "Hide password");
  return <div className="password-field">
    <input {...props} type={visible ? "text" : "password"} />
    <button type="button" className="icon-btn" title={visible ? hideLabel : showLabel} aria-label={visible ? hideLabel : showLabel} aria-pressed={visible} onClick={() => setVisible(!visible)}>{visible ? <EyeOff size={16} /> : <Eye size={16} />}</button>
  </div>;
}
