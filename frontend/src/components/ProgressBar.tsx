export type UsageTone = "good" | "warning" | "danger";

export function usageTone(value: number): UsageTone {
  const safe = Math.min(100, Math.max(0, value || 0));
  return safe >= 90 ? "danger" : safe >= 70 ? "warning" : "good";
}

export function ProgressBar({ value }: { value: number }) {
  const status = usageTone(value);
  return (
    <span className="progress-track" aria-hidden="true">
      <span className={`progress-fill ${status}`} style={{ width: `${Math.min(100, Math.max(0, value || 0))}%` }} />
    </span>
  );
}
