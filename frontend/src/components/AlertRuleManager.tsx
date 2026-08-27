import { Pencil, Plus, Save, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { AdminServer, AlertRule, AlertRuleInput } from "../types";
import { Checkbox } from "./Checkbox";
import { useDialog } from "./useDialog";

const emptyRule: AlertRuleInput = {
  name: "",
  metric: "cpu",
  threshold: 90,
  duration_minutes: 5,
  aggregation: "average",
  enabled: true,
  server_ids: [],
};

const metricLabels = {
  cpu: "CPU 使用率",
  memory: "内存使用率",
  disk: "磁盘使用率",
  net_in: "下行速度",
  net_out: "上行速度",
};

export function AlertRuleManager({ servers, onError, onNotice }: {
  servers: AdminServer[];
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [rules, setRules] = useState<AlertRule[]>([]);
  const [editing, setEditing] = useState<AlertRule | "new" | null>(null);
  const [form, setForm] = useState<AlertRuleInput>(emptyRule);
  const [allServers, setAllServers] = useState(true);
  const [busy, setBusy] = useState(false);
  const editorDialog = useDialog<HTMLElement>(editing !== null, () => setEditing(null));

  const load = useCallback(async () => {
    try { setRules((await api.alertRules()).rules); }
    catch (reason) { onError(reason instanceof Error ? reason.message : "读取告警规则失败"); }
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
      enabled: rule.enabled,
      server_ids: [...rule.server_ids],
    } : { ...emptyRule, server_ids: [] });
    setAllServers(!rule?.server_ids.length);
  }

  async function save() {
    setBusy(true);
    onError("");
    const input = { ...form, server_ids: allServers ? [] : form.server_ids };
    try {
      if (editing === "new") await api.createAlertRule(input);
      else if (editing) await api.updateAlertRule(editing.id, input);
      setEditing(null);
      await load();
      onNotice("告警规则已保存");
    } catch (reason) { onError(reason instanceof Error ? reason.message : "保存告警规则失败"); }
    finally { setBusy(false); }
  }

  async function remove(rule: AlertRule) {
    if (!window.confirm(`确认删除告警规则“${rule.name}”？`)) return;
    try { await api.deleteAlertRule(rule.id); await load(); onNotice("告警规则已删除"); }
    catch (reason) { onError(reason instanceof Error ? reason.message : "删除告警规则失败"); }
  }

  async function toggle(rule: AlertRule) {
    try {
      await api.updateAlertRule(rule.id, {
        name: rule.name,
        metric: rule.metric,
        threshold: rule.threshold,
        duration_minutes: rule.duration_minutes,
        aggregation: rule.aggregation,
        enabled: !rule.enabled,
        server_ids: rule.server_ids,
      });
      await load();
    } catch (reason) { onError(reason instanceof Error ? reason.message : "更新告警规则失败"); }
  }

  const unit = form.metric === "net_in" || form.metric === "net_out" ? "MiB/s" : "%";

  return <div className="alert-rule-manager">
    <div className="section-head"><div><h3>资源告警规则</h3><span>最多 20 条，可指定服务器和统计窗口</span></div><button type="button" className="primary-btn compact" onClick={() => open()}><Plus size={15} />新建规则</button></div>
    <div className="alert-rule-list">
      {rules.map((rule) => <div className="alert-rule-row" key={rule.id}>
        <Checkbox checked={rule.enabled} onChange={() => void toggle(rule)} ariaLabel={`${rule.name}启用状态`} />
        <div><strong>{rule.name}</strong><small>{metricLabels[rule.metric]} ≥ {rule.threshold} {rule.metric.startsWith("net_") ? "MiB/s" : "%"} · {rule.duration_minutes} 分钟{rule.aggregation === "continuous" ? "持续" : "平均"} · {rule.server_ids.length ? `${rule.server_ids.length} 台服务器` : "全部服务器"}</small></div>
        <button type="button" className="icon-btn" title="编辑规则" onClick={() => open(rule)}><Pencil size={15} /></button>
        <button type="button" className="icon-btn danger" title="删除规则" onClick={() => void remove(rule)}><Trash2 size={15} /></button>
      </div>)}
      {!rules.length ? <div className="list-empty">尚未配置资源告警规则</div> : null}
    </div>

    {editing ? <div className="submodal-backdrop" role="presentation" onMouseDown={editorDialog.onBackdropMouseDown}><section ref={editorDialog.dialogRef} className="alert-rule-editor glass-panel" role="dialog" aria-modal="true" aria-labelledby="alert-rule-editor-title" tabIndex={-1}>
      <header><div><span className="eyebrow">通知规则</span><h3 id="alert-rule-editor-title">{editing === "new" ? "新建资源告警" : "编辑资源告警"}</h3></div></header>
      <label><span>规则名称</span><input required maxLength={80} value={form.name} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} /></label>
      <div className="form-grid"><label><span>监控指标</span><select value={form.metric} onChange={(event) => setForm((current) => ({ ...current, metric: event.target.value as AlertRuleInput["metric"] }))}>{Object.entries(metricLabels).map(([value, label]) => <option value={value} key={value}>{label}</option>)}</select></label><label><span>阈值（{unit}）</span><input required type="number" min="0.01" max={unit === "%" ? 100 : 1000000} step="0.01" value={form.threshold} onChange={(event) => setForm((current) => ({ ...current, threshold: Number(event.target.value) }))} /></label></div>
      <div className="form-grid"><label><span>时间窗口（分钟）</span><input required type="number" min="1" max="1440" value={form.duration_minutes} onChange={(event) => setForm((current) => ({ ...current, duration_minutes: Number(event.target.value) }))} /></label><label><span>判断方式</span><select value={form.aggregation} onChange={(event) => setForm((current) => ({ ...current, aggregation: event.target.value as AlertRuleInput["aggregation"] }))}><option value="average">窗口平均值</option><option value="continuous">窗口内持续超限</option></select></label></div>
      <div className="settings-toggles"><label className="toggle-row"><b>启用规则</b><Checkbox checked={form.enabled} onChange={(value) => setForm((current) => ({ ...current, enabled: value }))} /></label><label className="toggle-row"><b>全部服务器</b><Checkbox checked={allServers} onChange={setAllServers} /></label></div>
      {!allServers ? <div className="alert-server-picker">{servers.map((server) => <label key={server.id}><Checkbox checked={form.server_ids.includes(server.id)} onChange={(checked) => setForm((current) => ({ ...current, server_ids: checked ? [...current.server_ids, server.id] : current.server_ids.filter((id) => id !== server.id) }))} /><span>{server.name}</span></label>)}</div> : null}
      <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => setEditing(null)}>取消</button><button type="button" className="primary-btn" onClick={() => void save()} disabled={busy || !form.name.trim() || (!allServers && !form.server_ids.length)}><Save size={15} />保存规则</button></div>
    </section></div> : null}
  </div>;
}
