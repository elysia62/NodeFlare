import { Pencil, Plus, Save, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import { ui, type UiLocale } from "../locale";
import type { AdminServer, AlertRule, AlertRuleInput } from "../types";
import { Checkbox } from "./Checkbox";
import { useDialog } from "./useDialog";

const emptyRule: AlertRuleInput = {
  name: "",
  metric: "cpu",
  threshold: 90,
  duration_minutes: 5,
  aggregation: "average",
  all_servers: true,
  enabled: true,
  server_ids: [],
};

const metricLabels = (locale: UiLocale | string | undefined) => ({
  cpu: ui(locale, "CPU 使用率", "CPU usage"),
  memory: ui(locale, "内存使用率", "Memory usage"),
  disk: ui(locale, "磁盘使用率", "Disk usage"),
  net_in: ui(locale, "下行速度", "Download speed"),
  net_out: ui(locale, "上行速度", "Upload speed"),
});

export function AlertRuleManager({ locale, servers, onError, onNotice }: {
  locale: UiLocale | string | undefined;
  servers: AdminServer[];
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [rules, setRules] = useState<AlertRule[]>([]);
  const [editing, setEditing] = useState<AlertRule | "new" | null>(null);
  const [form, setForm] = useState<AlertRuleInput>(emptyRule);
  const [busy, setBusy] = useState(false);
  const editorDialog = useDialog<HTMLElement>(editing !== null, () => setEditing(null));

  const load = useCallback(async () => {
    try { setRules((await api.alertRules()).rules); }
    catch (reason) { onError(reason instanceof Error ? reason.message : ui(locale, "读取告警规则失败", "Failed to load alert rules")); }
  }, [onError]);

  useEffect(() => { void load(); }, [load]);

  function open(rule?: AlertRule) {
    setEditing(rule ?? "new");
    setForm(rule ? {
      name: rule.name,
      metric: rule.metric,
      threshold: rule.threshold,
      duration_minutes: rule.duration_minutes,
      aggregation: rule.aggregation,
      all_servers: rule.all_servers,
      enabled: rule.enabled,
      server_ids: [...rule.server_ids],
    } : { ...emptyRule, server_ids: [] });
  }

  async function save() {
    setBusy(true);
    onError("");
    const input = { ...form, server_ids: form.all_servers ? [] : form.server_ids };
    try {
      if (editing === "new") await api.createAlertRule(input);
      else if (editing) await api.updateAlertRule(editing.id, input);
      setEditing(null);
      await load();
      onNotice(ui(locale, "告警规则已保存", "Alert rule saved"));
    } catch (reason) { onError(reason instanceof Error ? reason.message : ui(locale, "保存告警规则失败", "Failed to save the alert rule")); }
    finally { setBusy(false); }
  }

  async function remove(rule: AlertRule) {
    if (!window.confirm(ui(locale, `确认删除告警规则“${rule.name}”？`, `Delete the alert rule "${rule.name}"?`))) return;
    try { await api.deleteAlertRule(rule.id); await load(); onNotice(ui(locale, "告警规则已删除", "Alert rule deleted")); }
    catch (reason) { onError(reason instanceof Error ? reason.message : ui(locale, "删除告警规则失败", "Failed to delete the alert rule")); }
  }

  async function toggle(rule: AlertRule) {
    try {
      await api.updateAlertRule(rule.id, {
        name: rule.name,
        metric: rule.metric,
        threshold: rule.threshold,
        duration_minutes: rule.duration_minutes,
        aggregation: rule.aggregation,
        all_servers: rule.all_servers,
        enabled: !rule.enabled,
        server_ids: rule.server_ids,
      });
      await load();
    } catch (reason) { onError(reason instanceof Error ? reason.message : ui(locale, "更新告警规则失败", "Failed to update the alert rule")); }
  }

  const unit = form.metric === "net_in" || form.metric === "net_out" ? "MiB/s" : "%";

  return <div className="alert-rule-manager">
    <div className="section-head"><div><h3>{ui(locale, "资源告警规则", "Resource alert rules")}</h3><span>{ui(locale, "最多 20 条，可指定服务器和统计窗口", "Up to 20 rules; can target specific servers and windows")}</span></div><button type="button" className="primary-btn compact" onClick={() => open()}><Plus size={15} />{ui(locale, "新建规则", "New rule")}</button></div>
    <div className="alert-rule-list">
      {rules.map((rule) => <div className="alert-rule-row" key={rule.id}>
        <Checkbox checked={rule.enabled} onChange={() => void toggle(rule)} ariaLabel={ui(locale, `${rule.name}启用状态`, `${rule.name} enabled state`)} />
        <div><strong>{rule.name}</strong><small>{metricLabels(locale)[rule.metric]} ≥ {rule.threshold} {rule.metric.startsWith("net_") ? "MiB/s" : "%"} · {ui(locale, `${rule.duration_minutes} 分钟`, `${rule.duration_minutes} min`)}{rule.aggregation === "continuous" ? ui(locale, "持续", " continuous") : ui(locale, "平均", " average")} · {rule.all_servers ? ui(locale, "全部服务器", "All servers") : ui(locale, `${rule.server_ids.length} 台服务器`, `${rule.server_ids.length} server(s)`)}</small></div>
        <button type="button" className="icon-btn" title={ui(locale, "编辑规则", "Edit rule")} onClick={() => open(rule)}><Pencil size={15} /></button>
        <button type="button" className="icon-btn danger" title={ui(locale, "删除规则", "Delete rule")} onClick={() => void remove(rule)}><Trash2 size={15} /></button>
      </div>)}
      {!rules.length ? <div className="list-empty">{ui(locale, "尚未配置资源告警规则", "No resource alert rules yet")}</div> : null}
    </div>

    {editing ? <div className="submodal-backdrop" role="presentation" onMouseDown={editorDialog.onBackdropMouseDown}><section ref={editorDialog.dialogRef} className="alert-rule-editor glass-panel" role="dialog" aria-modal="true" aria-labelledby="alert-rule-editor-title" tabIndex={-1}>
      <header><div><span className="eyebrow">{ui(locale, "通知规则", "Notification rule")}</span><h3 id="alert-rule-editor-title">{editing === "new" ? ui(locale, "新建资源告警", "New resource alert") : ui(locale, "编辑资源告警", "Edit resource alert")}</h3></div></header>
      <label><span>{ui(locale, "规则名称", "Rule name")}</span><input required maxLength={80} value={form.name} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} /></label>
      <div className="form-grid"><label><span>{ui(locale, "监控指标", "Metric")}</span><select value={form.metric} onChange={(event) => setForm((current) => ({ ...current, metric: event.target.value as AlertRuleInput["metric"] }))}>{Object.entries(metricLabels(locale)).map(([value, label]) => <option value={value} key={value}>{label}</option>)}</select></label><label><span>{ui(locale, `阈值（${unit}）`, `Threshold (${unit})`)}</span><input required type="number" min="0.01" max={unit === "%" ? 100 : 1000000} step="0.01" value={form.threshold} onChange={(event) => setForm((current) => ({ ...current, threshold: Number(event.target.value) }))} /></label></div>
      <div className="form-grid"><label><span>{ui(locale, "时间窗口（分钟）", "Time window (min)")}</span><input required type="number" min="1" max="1440" value={form.duration_minutes} onChange={(event) => setForm((current) => ({ ...current, duration_minutes: Number(event.target.value) }))} /></label><label><span>{ui(locale, "判断方式", "Evaluation")}</span><select value={form.aggregation} onChange={(event) => setForm((current) => ({ ...current, aggregation: event.target.value as AlertRuleInput["aggregation"] }))}><option value="average">{ui(locale, "窗口平均值", "Window average")}</option><option value="continuous">{ui(locale, "窗口内持续超限", "Continuously above threshold")}</option></select></label></div>
      <div className="settings-toggles"><label className="toggle-row"><b>{ui(locale, "启用规则", "Enable rule")}</b><Checkbox checked={form.enabled} onChange={(value) => setForm((current) => ({ ...current, enabled: value }))} /></label><label className="toggle-row"><b>{ui(locale, "全部服务器", "All servers")}</b><Checkbox checked={form.all_servers} onChange={(value) => setForm((current) => ({ ...current, all_servers: value }))} /></label></div>
      {!form.all_servers ? <div className="alert-server-picker">{servers.map((server) => <label key={server.id}><Checkbox checked={form.server_ids.includes(server.id)} onChange={(checked) => setForm((current) => ({ ...current, server_ids: checked ? [...current.server_ids, server.id] : current.server_ids.filter((id) => id !== server.id) }))} /><span>{server.name}</span></label>)}</div> : null}
      <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => setEditing(null)}>{ui(locale, "取消", "Cancel")}</button><button type="button" className="primary-btn" onClick={() => void save()} disabled={busy || !form.name.trim() || (!form.all_servers && !form.server_ids.length)}><Save size={15} />{ui(locale, "保存规则", "Save rule")}</button></div>
    </section></div> : null}
  </div>;
}
