import type { DragEvent } from "react";
import { ChevronDown, ChevronUp, Download, GripVertical, Pencil, Plus, Trash2 } from "lucide-react";
import { ui } from "../../locale";
import type { AdminServer } from "../../types";
import { Checkbox } from "../Checkbox";
import { Flag } from "../Flag";

/** Public IP badges plus the reported Agent version for one server row. */
export function ServerIpMeta({ server, agentVersion, onCopy, locale }: {
  server: AdminServer;
  agentVersion: string | null;
  onCopy: (ip: string) => void;
  locale: string;
}) {
  const entries = [
    { family: "v4" as const, ip: server.ip_v4 || "" },
    { family: "v6" as const, ip: server.ip_v6 || "" },
  ].filter((entry) => entry.ip);
  return (
    <div className="server-name-meta">
      {entries.map((entry) => (
        <span className="server-ip-entry" key={entry.family}>
          <span className={`ip-badge ${entry.family}`}>{entry.family === "v6" ? "IPv6" : "IPv4"}</span>
          <button type="button" className="ip-value" title={ui(locale, `点击复制：${entry.ip}`, `Click to copy: ${entry.ip}`)} onClick={() => onCopy(entry.ip)}>{entry.ip}</button>
        </span>
      ))}
      {!entries.length ? <span className="meta-item">{ui(locale, "尚未探测到公网 IP", "No public IP detected yet")}</span> : null}
      <span className="meta-dot">·</span>
      <span className="meta-item">{agentVersion ? `Agent v${agentVersion}` : ui(locale, "Agent 未上报版本", "Agent version not reported")}</span>
    </div>
  );
}

/** Draggable server list with batch selection. */
export function ServersTab({
  locale,
  servers,
  busy,
  selectedIds,
  allSelected,
  draggingId,
  onCopyIp,
  onAdd,
  onToggleSelected,
  onToggleAll,
  onRemoveSelected,
  onMove,
  onStartDrag,
  onEndDrag,
  onDrop,
  onInstallCommand,
  onEdit,
  onRemove,
}: {
  locale: string;
  servers: AdminServer[];
  busy: boolean;
  selectedIds: string[];
  allSelected: boolean;
  draggingId: string;
  onCopyIp: (ip: string) => void;
  onAdd: () => void;
  onToggleSelected: (id: string) => void;
  onToggleAll: () => void;
  onRemoveSelected: () => void;
  onMove: (index: number, offset: number) => void;
  onStartDrag: (event: DragEvent<HTMLButtonElement>, id: string) => void;
  onEndDrag: () => void;
  onDrop: (event: DragEvent<HTMLDivElement>, targetId: string) => void;
  onInstallCommand: (server: AdminServer) => void;
  onEdit: (server: AdminServer) => void;
  onRemove: (server: AdminServer) => void;
}) {
  return (
    <div className="admin-section">
      <div className="section-head"><div><h3>{ui(locale, "监控节点", "Monitored servers")}</h3><span>{ui(locale, `${servers.length} 个节点 · 可拖动上下排序`, `${servers.length} server(s) · drag to reorder`)}</span></div><div className="section-actions"><button className="primary-btn compact" onClick={onAdd}><Plus size={15} />{ui(locale, "添加", "Add")}</button></div></div>
      <div className="batch-toolbar"><label className="select-all"><Checkbox checked={allSelected} onChange={onToggleAll} />{ui(locale, "全选", "Select all")}</label>{selectedIds.length ? <button className="danger-btn compact" onClick={onRemoveSelected}><Trash2 size={15} />{ui(locale, `删除选中 (${selectedIds.length})`, `Delete selected (${selectedIds.length})`)}</button> : <span>{ui(locale, "批量操作", "Batch actions")}</span>}</div>
      <div className="server-list">
        {servers.map((server, index) => (
          <div className={`server-row ${draggingId === server.id ? "dragging" : ""}`} key={server.id} onDragOver={(event) => { event.preventDefault(); event.dataTransfer.dropEffect = "move"; }} onDrop={(event) => onDrop(event, server.id)}>
            <button type="button" className="drag-handle" draggable onDragStart={(event) => onStartDrag(event, server.id)} onDragEnd={onEndDrag} title={ui(locale, `拖动排序：${server.name}`, `Drag to reorder: ${server.name}`)}><GripVertical size={15} /></button>
            <Checkbox checked={selectedIds.includes(server.id)} onChange={() => onToggleSelected(server.id)} ariaLabel={ui(locale, `选择 ${server.name}`, `Select ${server.name}`)} />
            <div className="server-name"><div className="server-name-main"><Flag region={server.region} size={17} /><strong>{server.name}</strong></div><ServerIpMeta server={server} agentVersion={server.agent_version} onCopy={onCopyIp} locale={locale} /></div>
            <div className="row-actions"><button className="icon-btn" disabled={index === 0} onClick={() => onMove(index, -1)} title={ui(locale, "上移", "Move up")}><ChevronUp size={15} /></button><button className="icon-btn" disabled={index === servers.length - 1} onClick={() => onMove(index, 1)} title={ui(locale, "下移", "Move down")}><ChevronDown size={15} /></button><button className="icon-btn" disabled={busy} onClick={() => onInstallCommand(server)} title={ui(locale, "下载 Agent", "Download Agent")}><Download size={15} /></button><button className="icon-btn" onClick={() => onEdit(server)} title={ui(locale, "编辑节点", "Edit server")}><Pencil size={15} /></button><button className="icon-btn danger" onClick={() => onRemove(server)} title={ui(locale, "删除节点", "Delete server")}><Trash2 size={15} /></button></div>
          </div>
        ))}
        {!servers.length && !busy ? <div className="list-empty">{ui(locale, "暂无节点", "No servers yet")}</div> : null}
      </div>
    </div>
  );
}
