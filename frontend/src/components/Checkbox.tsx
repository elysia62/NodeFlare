import { Check } from "lucide-react";

export function Checkbox({
  checked,
  onChange,
  ariaLabel,
  disabled = false,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  ariaLabel?: string;
  disabled?: boolean;
}) {
  return <span className="checkbox-control"><input aria-label={ariaLabel} type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} /><Check aria-hidden="true" /></span>;
}
