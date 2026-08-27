export function ProgressBar({ value }: { value: number }) {
  const safe = Math.min(100, Math.max(0, value || 0));
  const status = safe >= 90 ? "danger" : safe >= 70 ? "warning" : "good";
  // 纯装饰：调用方 Metric 已经把同一个百分比作为可见文本渲染在旁边，再报一遍是重复。
  // aria-hidden 明说它不进无障碍树。原来这里挂 aria-label，但无 role 的 span 是 generic，
  // 规范禁止给它命名，那个标签从来没生效过。
  return (
    <span className="progress-track" aria-hidden="true">
      <span className={`progress-fill ${status}`} style={{ width: `${safe}%` }} />
    </span>
  );
}

