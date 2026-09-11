import { Checkbox } from "../Checkbox";
import { type ThemeSettingField, type ThemeSettingValue } from "../types";

export function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (value: boolean) => void }) {
  return <label className="toggle-row"><span><b>{label}</b></span><Checkbox checked={checked} onChange={onChange} /></label>;
}

export function ThemeOption({ field, value, onChange }: { field: ThemeSettingField; value: ThemeSettingValue | undefined; onChange: (value: ThemeSettingValue) => void }) {
  if (field.type === "toggle") {
    return <Toggle label={field.label} checked={typeof value === "boolean" ? value : false} onChange={onChange} />;
  }
  if (field.type === "textarea") {
    return <label className="theme-option"><span>{field.label}</span><textarea rows={3} maxLength={500} placeholder={field.placeholder} value={typeof value === "string" ? value : ""} onChange={(event) => onChange(event.target.value)} /></label>;
  }
  if (field.type === "select") {
    return <label className="theme-option"><span>{field.label}</span><select value={typeof value === "string" ? value : ""} onChange={(event) => onChange(event.target.value)}>{field.options?.map((option) => <option value={option.value} key={option.value}>{option.label}</option>)}</select></label>;
  }
  if (field.type === "number") {
    return <label className="theme-option"><span>{field.label}</span><input type="number" min={field.min} max={field.max} step={field.step} value={typeof value === "number" ? value : ""} onChange={(event) => onChange(Number(event.target.value))} /></label>;
  }
  return <label className={`theme-option ${field.type === "color" ? "theme-color-option" : ""}`}><span>{field.label}</span><input type={field.type} maxLength={field.type === "color" ? undefined : 500} placeholder={field.placeholder} value={typeof value === "string" ? value : field.type === "color" ? "#0f766e" : ""} onChange={(event) => onChange(event.target.value)} /></label>;
}
