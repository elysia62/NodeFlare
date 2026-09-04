export function ProgressBar({ value }: { value: number }) {
  const safe = Math.min(100, Math.max(0, value || 0));
  const status = safe >= 90 ? "danger" : safe >= 70 ? "warning" : "good";
  return (
    <span className="progress-track" aria-hidden="true">
      <span className={`progress-fill ${status}`} style={{ width: `${safe}%` }} />
    </span>
  );
}
