import { Pencil, Plus, RadioTower, Save, Search, Trash2 } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { api } from "../api";
import { ui, type UiLocale } from "../locale";
import type { AdminServer, LatencyTask, LatencyTaskInput } from "../types";
import { Checkbox } from "./Checkbox";
import { useDialog } from "./useDialog";

type TaskForm = Omit<LatencyTaskInput, "port">;

const emptyTask: TaskForm = {
  name: "",
  task_type: "icmp",
  target: "",
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

function parseTarget(value: string, type: LatencyTask["task_type"]): Pick<LatencyTaskInput, "target" | "port"> | null {
  const target = value.trim();
  if (type === "icmp") return validHost(target) ? { target, port: null } : null;
  const match = /^([^:\s]+)(?::(\d{1,5}))?$/.exec(target);
  if (!match || !validHost(match[1])) return null;
  const port = Number(match[2] ?? 80);
  return port >= 1 && port <= 65535 ? { target: match[1], port } : null;
}

export function LatencyManager({
  locale,
  servers,
  onError,
  onNotice,
}: {
  locale: UiLocale | string | undefined;
  servers: AdminServer[];
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}) {
  const [tasks, setTasks] = useState<LatencyTask[]>([]);
  const [busy, setBusy] = useState(true);
  const [editing, setEditing] = useState<LatencyTask | "new" | null>(null);
  const [form, setForm] = useState<TaskForm>(emptyTask);
  const [query, setQuery] = useState("");
  const editorDialog = useDialog<HTMLFormElement>(editing !== null, () => setEditing(null));

  const load = useCallback(async () => {
    setBusy(true);
    onError("");
    try {
      setTasks((await api.latencyTasks()).tasks);
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "读取延迟任务失败", "Failed to load latency tasks"));
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
      target: task.task_type === "tcp" ? `${task.target}:${task.port ?? 80}` : task.target,
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
    const target = parseTarget(form.target, form.task_type);
    if (!target) {
      onError(form.task_type === "icmp" ? ui(locale, "ICMP 节点应为公网域名或公网 IPv4，不使用端口", "An ICMP target must be a public hostname or public IPv4 address without a port") : ui(locale, "TCP 节点格式应为域名:端口或公网 IPv4:端口，端口范围为 1 至 65535", "A TCP target must be host:port or public IPv4:port with port 1-65535"));
      return;
    }
    const input: LatencyTaskInput = { ...form, ...target };
    setBusy(true);
    onError("");
    try {
      if (editing === "new") await api.createLatencyTask(input);
      else if (editing) await api.updateLatencyTask(editing.id, input);
      setEditing(null);
      await load();
      onNotice(editing === "new" ? ui(locale, "延迟任务已添加", "Latency task added") : ui(locale, "延迟任务已更新", "Latency task updated"));
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "保存延迟任务失败", "Failed to save the latency task"));
    } finally {
      setBusy(false);
    }
  }

  async function remove(task: LatencyTask) {
    if (!window.confirm(ui(locale, `确认删除延迟任务“${task.name}”及其历史结果？`, `Delete the latency task "${task.name}" and its history?`))) return;
    setBusy(true);
    onError("");
    try {
      await api.deleteLatencyTask(task.id);
      await load();
      onNotice(ui(locale, "延迟任务已删除", "Latency task deleted"));
    } catch (reason) {
      onError(reason instanceof Error ? reason.message : ui(locale, "删除延迟任务失败", "Failed to delete the latency task"));
    } finally {
      setBusy(false);
    }
  }

  const allSelected = servers.length > 0 && form.server_ids.length === servers.length;

  return <div className="admin-section latency-section">
    <div className="section-head">
      <div><h3>{ui(locale, "延迟任务", "Latency tasks")}</h3><span>{ui(locale, `${tasks.length} 个任务 · TCP / ICMP`, `${tasks.length} task(s) · TCP / ICMP`)}</span></div>
      <button className="primary-btn compact" type="button" onClick={() => open()}><Plus size={15} />{ui(locale, "添加", "Add")}</button>
    </div>
    <div className="latency-task-list">
      {tasks.map((task) => <div className="latency-task-row" key={task.id}>
        <span className={`latency-type ${task.task_type}`}><RadioTower size={13} />{task.task_type.toUpperCase()}</span>
        <div className="latency-task-name"><strong>{task.name}</strong><small>{task.target}{task.port ? `:${task.port}` : ""}</small></div>
        <span className="latency-task-meta">{task.interval_seconds}s</span>
        <span className="latency-task-meta">{ui(locale, `${task.server_ids.length} 个节点`, `${task.server_ids.length} server(s)`)}</span>
        <div className="row-actions"><button className="icon-btn" type="button" onClick={() => open(task)} title={ui(locale, "编辑延迟任务", "Edit latency task")}><Pencil size={15} /></button><button className="icon-btn danger" type="button" onClick={() => void remove(task)} title={ui(locale, "删除延迟任务", "Delete latency task")}><Trash2 size={15} /></button></div>
      </div>)}
      {!tasks.length && !busy ? <div className="list-empty">{ui(locale, "暂无延迟任务", "No latency tasks")}</div> : null}
      {busy && !tasks.length ? <div className="list-empty">{ui(locale, "正在读取延迟任务", "Loading latency tasks")}</div> : null}
    </div>

    {editing ? <div className="submodal-backdrop" role="presentation" onMouseDown={editorDialog.onBackdropMouseDown}><form ref={editorDialog.dialogRef} className="latency-editor glass-panel" role="dialog" aria-modal="true" aria-labelledby="latency-editor-title" tabIndex={-1} onSubmit={save}>
      <header><div><span className="eyebrow">{ui(locale, "延迟检测", "Latency check")}</span><h3 id="latency-editor-title">{editing === "new" ? ui(locale, "添加任务", "Add task") : ui(locale, `编辑 · ${editing.name}`, `Edit · ${editing.name}`)}</h3></div></header>
      <div className="form-grid"><label><span>{ui(locale, "名称", "Name")}</span><input autoFocus required maxLength={80} value={form.name} onChange={(event) => setForm((current) => ({ ...current, name: event.target.value }))} /></label><label><span>{ui(locale, "类型", "Type")}</span><div className="segmented task-type-control"><button type="button" className={form.task_type === "icmp" ? "active" : ""} onClick={() => setForm((current) => ({ ...current, task_type: "icmp", target: parseTarget(current.target, current.task_type)?.target ?? current.target }))}>ICMP</button><button type="button" className={form.task_type === "tcp" ? "active" : ""} onClick={() => setForm((current) => ({ ...current, task_type: "tcp" }))}>TCP</button></div></label></div>
      <div className="form-grid latency-target-row"><label><span>{ui(locale, "节点", "Target")}</span><input required maxLength={form.task_type === "tcp" ? 56 : 50} value={form.target} onChange={(event) => setForm((current) => ({ ...current, target: event.target.value }))} placeholder={form.task_type === "tcp" ? "hb-cm-dualstack.ip.zstaticcdn.com:80" : "1.1.1.1"} /></label><label><span>{ui(locale, "检测间隔（秒）", "Interval (s)")}</span><input type="number" min="30" max="3600" required value={form.interval_seconds} onChange={(event) => setForm((current) => ({ ...current, interval_seconds: Number(event.target.value) }))} /></label></div>
      <div className="server-picker">
        <div className="server-picker-head"><strong>{ui(locale, "服务器", "Servers")}</strong><span>{ui(locale, `已选 ${form.server_ids.length} / 共 ${servers.length}`, `Selected ${form.server_ids.length} / ${servers.length}`)}</span><button type="button" onClick={() => setForm((current) => ({ ...current, server_ids: allSelected ? [] : servers.map((server) => server.id) }))}>{allSelected ? ui(locale, "取消全选", "Deselect all") : ui(locale, "全选", "Select all")}</button></div>
        <div className="server-picker-search"><Search size={16} /><input aria-label={ui(locale, "搜索服务器", "Search servers")} placeholder={ui(locale, "搜索", "Search")} value={query} onChange={(event) => setQuery(event.target.value)} /></div>
        <div className="server-picker-list">
          {visibleServers.map((server) => <label className="server-picker-row" key={server.id}><Checkbox checked={form.server_ids.includes(server.id)} onChange={() => toggleServer(server.id)} /><span><strong>{server.name}</strong></span></label>)}
          {!visibleServers.length ? <div className="server-picker-empty">{ui(locale, "没有匹配的服务器", "No matching servers")}</div> : null}
        </div>
      </div>
      <label className="toggle-row"><span><b>{ui(locale, "默认分配给新服务器", "Assign to new servers by default")}</b></span><Checkbox checked={form.default_enabled} onChange={(checked) => setForm((current) => ({ ...current, default_enabled: checked }))} /></label>
      <div className="form-actions"><button type="button" className="secondary-btn" onClick={() => setEditing(null)}>{ui(locale, "取消", "Cancel")}</button><button className="primary-btn" disabled={busy}><Save size={15} />{busy ? ui(locale, "保存中", "Saving") : ui(locale, "保存任务", "Save task")}</button></div>
    </form></div> : null}
  </div>;
}
