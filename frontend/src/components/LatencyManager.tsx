import { Pencil, Plus, RadioTower, Save, Search, Trash2 } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import type { AdminServer, LatencyTask, LatencyTaskInput } from "../types";
import { Checkbox } from "./Checkbox";
import { useDialog } from "./useDialog";

const emptyTask: LatencyTaskInput = {
  name: "",
  task_type: "icmp",
  target: "",
  port: null,
  interval_seconds: 60,
  default_enabled: false,
  server_ids: [],
};

function validHost(value: string) {
  const name = value.trim();
  if (!name || name.length > 50 || /\s|:|[\/@?#\\\[\]]/.test(name)) return false;
  if (!name || name.startsWith(".") || name.endsWith(".")) return false;
  const labels = name.split(".");
  if (labels.length === 4 && labels.every((part) => /^\d+$/.test(part))) {
    const octets = labels.map(Number);
    if (octets.some((part) => part < 0 || part > 255)) return false;
    const [a, b, c] = octets;
    return !(a === 0 || a === 10 || a === 127 || (a === 100 && b >= 64 && b <= 127)
      || (a === 169 && b === 254) || (a === 172 && b >= 16 && b <= 31)
      || (a === 192 && b === 0 && (c === 0 || c === 2)) || (a === 192 && b === 168)
      || (a === 198 && (b === 18 || b === 19 || (b === 51 && c === 100)))
      || (a === 203 && b === 0 && c === 113) || a >= 224);
  }
  const lower = name.toLowerCase();
  if (labels.length < 2 || ["local", "localhost", "internal", "lan", "localdomain"].some((suffix) => lower === suffix || lower.endsWith(`.${suffix}`)) || lower === "home.arpa" || lower.endsWith(".home.arpa")) return false;
  return labels.every((part) => /^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$/.test(part));
}

export function LatencyManager({
  servers,
  onError,
  onNotice,
}: {
  servers: AdminServer[];
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [tasks, setTasks] = useState<LatencyTask[]>([]);
  const [busy, setBusy] = useState(true);
  const [editing, setEditing] = useState<LatencyTask | "new" | null>(null);
  const [form, setForm] = useState<LatencyTaskInput>(emptyTask);
  const [query, setQuery] = useState("");
  const editorDialog = useDialog<HTMLFormElement>(editing !== null, () => setEditing(null));

  const load = useCallback(async () => {
    setBusy(true);
    onError("");
    try {
      setTasks((await api.latencyTasks()).tasks);
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : "读取延迟任务失败");
    } finally {
      setBusy(false);
    }
  }, [onError]);

  useEffect(() => { void load(); }, [load]);

  const visibleServers = useMemo(() => {
    const keyword = query.trim().toLowerCase();
    if (!keyword) return servers;
    return servers.filter((server) => `${server.name} ${server.region} ${server.group_name} ${server.last_ip}`.toLowerCase().includes(keyword));
  }, [query, servers]);

  function open(task?: LatencyTask) {
    setEditing(task ?? "new");
    setForm(task ? {
      name: task.name,
      task_type: task.task_type,
      target: task.target,
      port: task.port,
      interval_seconds: task.interval_seconds,
      default_enabled: task.default_enabled,
      server_ids: [...task.server_ids],
    } : { ...emptyTask, server_ids: [] });
    setQuery("");
    onError("");
  }

  function toggleServer(id: string) {
    setForm((current) => ({
      ...current,
      server_ids: current.server_ids.includes(id)
        ? current.server_ids.filter((serverId) => serverId !== id)
        : [...current.server_ids, id],
    }));
  }

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!validHost(form.target) || (form.task_type === "tcp" && (!form.port || form.port < 1 || form.port > 65535)) || (form.task_type === "icmp" && form.port !== null)) {
      onError(form.task_type === "icmp" ? "ICMP 节点应为公网域名或公网 IPv4，不使用端口" : "TCP 节点应为公网域名或公网 IPv4，并填写 1 至 65535 的端口");
      return;
    }
    setBusy(true);
    onError("");
    try {
      if (editing === "new") await api.createLatencyTask(form);
      else if (editing) await api.updateLatencyTask(editing.id, form);
      setEditing(null);
      await load();
      onNotice(editing === "new" ? "延迟任务已添加" : "延迟任务已更新");
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : "保存延迟任务失败");
    } finally {
      setBusy(false);
    }
  }

  async function remove(task: LatencyTask) {
    if (!window.confirm(`确认删除延迟任务“${task.name}”及其历史结果？`)) return;
    setBusy(true);
    onError("");
    try {
      await api.deleteLatencyTask(task.id);
      await load();
      onNotice("延迟任务已删除");
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : "删除延迟任务失败");
    } finally {
      setBusy(false);
    }
  }

  const allSelected = servers.length > 0 && form.server_ids.length === servers.length;

  return <div className="admin-section latency-section">
    <div className="section-head">
      <div><h3>延迟任务</h3><span>{tasks.length} 个任务 · TCP / ICMP</span></div>
      <button className="primary-btn compact" type="button" onClick={() => open()}><Plus size={15} />添加</button>
    </div>
    <div className="latency-task-list">
      {tasks.map((task) => <div className="latency-task-row" key={task.id}>
        <span className={`latency-type ${task.task_type}`}><RadioTower size={13} />{task.task_type.toUpperCase()}</span>
        <div className="latency-task-name"><strong>{task.name}</strong><small>{task.target}{task.port ? `:${task.port}` : ""}</small></div>
        <span className="latency-task-meta">{task.interval_seconds}s</span>
        <span className="latency-task-meta">{task.server_ids.length} 个节点{task.default_enabled ? " · 默认" : ""}</span>
        <div className="row-actions"><button className="icon-btn" type="button" onClick={() => open(task)} title="编辑延迟任务"><Pencil size={15} /></button><button className="icon-btn danger" type="button" onClick={() => void remove(task)} title="删除延迟任务"><Trash2 size={15} /></button></div>
      </div>)}
      {!tasks.length && !busy ? <div className="list-empty">暂无延迟任务</div> : null}
      {busy && !tasks.length ? <div className="list-empty">正在读取延迟任务</div> : null}
    </div>

    {editing ? <div className="submodal-backdrop" role="presentation" onMouseDown={editorDialog.onBackdropMouseDown}><form ref={editorDialog.dialogRef} className="latency-editor glass-panel" role="dialog" aria-modal="true" aria-labelledby="latency-editor-title" tabIndex={-1} onSubmit={save}>
      <header><div><span className="eyebrow">延迟检测</span><h3 id="latency-editor-title">{editing === "new" ? "添加任务" : `编辑 · ${editing.name}`}</h3></div></header>
      <div className="form-grid"><label><span>名称</span><input autoFocus required maxLength={80} value={form.name} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} /></label><label><span>类型</span><div className="segmented task-type-control"><button type="button" className={form.task_type === "icmp" ? "active" : ""} onClick={() => setForm((current) => ({ ...current, task_type: "icmp", port: null }))}>ICMP</button><button type="button" className={form.task_type === "tcp" ? "active" : ""} onClick={() => setForm((current) => ({ ...current, task_type: "tcp", port: current.port ?? 80 }))}>TCP</button></div></label></div>
      <div className="form-grid"><label><span>节点</span><input required maxLength={50} value={form.target} onChange={(event) => setForm((current) => ({ ...current, target: event.target.value }))} placeholder={form.task_type === "tcp" ? "example.com" : "1.1.1.1"} /></label>{form.task_type === "tcp" ? <label><span>端口</span><input type="number" min="1" max="65535" required value={form.port ?? ""} onChange={(event) => setForm((current) => ({ ...current, port: event.target.value ? Number(event.target.value) : null }))} placeholder="80" /></label> : <div />}</div>
      <label><span>检测间隔（秒）</span><input type="number" min="30" max="3600" required value={form.interval_seconds} onChange={(event) => setForm((current) => ({ ...current, interval_seconds: Number(event.target.value) }))} /></label>
      <div className="server-picker">
        <div className="server-picker-head"><strong>服务器</strong><span>已选 {form.server_ids.length} / 共 {servers.length}</span><button type="button" onClick={() => setForm((current) => ({ ...current, server_ids: allSelected ? [] : servers.map((server) => server.id) }))}>{allSelected ? "取消全选" : "全选"}</button></div>
        <div className="server-picker-search"><Search size={16} /><input aria-label="搜索服务器" placeholder="搜索" value={query} onChange={(event) => setQuery(event.target.value)} /></div>
        <div className="server-picker-list">
          {visibleServers.map((server) => <label className="server-picker-row" key={server.id}><Checkbox checked={form.server_ids.includes(server.id)} onChange={() => toggleServer(server.id)} /><span><strong>{server.name}</strong><small>{server.last_ip || "尚未上报 IP"}</small></span></label>)}
          {!visibleServers.length ? <div className="server-picker-empty">没有匹配的服务器</div> : null}
        </div>
      </div>
      <label className="toggle-row"><span><b>默认分配给新服务器</b></span><Checkbox checked={form.default_enabled} onChange={(checked) => setForm((current) => ({ ...current, default_enabled: checked }))} /></label>
      <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => setEditing(null)}>取消</button><button className="primary-btn" disabled={busy}><Save size={15} />{busy ? "保存中" : "保存任务"}</button></div>
    </form></div> : null}
  </div>;
}
