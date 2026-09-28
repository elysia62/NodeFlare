import { ChevronDown, ChevronUp, Download, Pencil, Plus, Trash2 } from "lucide-react";
import { ui } from "../../locale";
import type { AdminServer } from "../../types";
import { Checkbox } from "../Checkbox";
import { Flag } from "../Flag";
import { formatDate } from "./shared";

/** Copyable IP addresses for one server row. */
function ServerIpAddresses({ server, onCopy, locale }: {
  server: AdminServer;
  onCopy: (ip: string) => void;
  locale: string;
}) {
  const entries = [
    { family: "v4" as const, ip: server.ip_v4 || "" },
    { family: "v6" as const, ip: server.ip_v6 || "" },
  ].filter((entry) => entry.ip);
  if (!entries.length && server.last_ip) {
    entries.push({ family: server.last_ip.includes(":") ? "v6" : "v4", ip: server.last_ip });
  }
  return (
    <div className="server-ip-addresses">
      {entries.map((entry) => (
        <span className="server-ip-entry" key={entry.family}>
          <span className={`ip-badge ${entry.family}`}>{entry.family === "v6" ? "IPv6" : "IPv4"}</span>
          <button type="button" className="ip-value" title={ui(locale, `点击复制：${entry.ip}`, `Click to copy: ${entry.ip}`)} onClick={() => onCopy(entry.ip)}>{entry.ip}</button>
        </span>
      ))}
      {!entries.length ? <span>{ui(locale, "未上报", "Not reported")}</span> : null}
    </div>
  );
}

/** Server list with batch selection and move controls. */
export function ServersTab({
  locale,
  servers,
  busy,
  selectedIds,
  allSelected,
  onCopyIp,
  onAdd,
  onToggleSelected,
  onToggleAll,
  onRemoveSelected,
  onMove,
  onInstallCommand,
  onEdit,
  onRemove,
}: {
  locale: string;
  servers: AdminServer[];
  busy: boolean;
  selectedIds: string[];
  allSelected: boolean;
  onCopyIp: (ip: string) => void;
  onAdd: () => void;
  onToggleSelected: (id: string) => void;
  onToggleAll: () => void;
  onRemoveSelected: () => void;
  onMove: (index: number, offset: number) => void;
  onInstallCommand: (server: AdminServer) => void;
  onEdit: (server: AdminServer) => void;
  onRemove: (server: AdminServer) => void;
}) {
  return (
    <div className="admin-section">
      <div className="section-head"><div><h3>{ui(locale, "监控节点", "Monitored servers")}</h3><span>{ui(locale, `${servers.length} 个节点`, `${servers.length} server(s)`)}</span></div><div className="section-actions"><button className="primary-btn compact" onClick={onAdd}><Plus size={15} />{ui(locale, "添加", "Add")}</button></div></div>
      <div className="server-list">
        <div className="server-list-header">
          <label className="select-all"><Checkbox checked={allSelected} onChange={onToggleAll} />{ui(locale, "全选", "Select all")}</label>
          <span>{ui(locale, "IP 地址", "IP address")}</span>
          <span>{ui(locale, "Agent 版本", "Agent version")}</span>
          <span>{ui(locale, "到期时间", "Expiry date")}</span>
          <div className="server-batch-actions">{selectedIds.length ? <button className="danger-btn compact" onClick={onRemoveSelected}><Trash2 size={15} />{ui(locale, `删除选中 (${selectedIds.length})`, `Delete selected (${selectedIds.length})`)}</button> : <span>{ui(locale, "批量操作", "Batch actions")}</span>}</div>
        </div>
        {servers.map((server, index) => (
          <div className="server-row" key={server.id}>
            <Checkbox checked={selectedIds.includes(server.id)} onChange={() => onToggleSelected(server.id)} ariaLabel={ui(locale, `选择 ${server.name}`, `Select ${server.name}`)} />
            <div className="server-name"><div className="server-name-main"><Flag region={server.region} size={17} /><strong title={server.name}>{server.name}</strong></div></div>
            <ServerIpAddresses server={server} onCopy={onCopyIp} locale={locale} />
            <span className="server-agent-version" title={server.agent_version || undefined}>{server.agent_version ? `v${server.agent_version.replace(/^v/, "")}` : ui(locale, "未上报", "Not reported")}</span>
            <span className="server-expiry">{formatDate(server.expires_at) || ui(locale, "未设置", "Not set")}</span>
            <div className="row-actions"><button className="icon-btn" disabled={index === 0} onClick={() => onMove(index, -1)} title={ui(locale, "上移", "Move up")}><ChevronUp size={15} /></button><button className="icon-btn" disabled={index === servers.length - 1} onClick={() => onMove(index, 1)} title={ui(locale, "下移", "Move down")}><ChevronDown size={15} /></button><button className="icon-btn" disabled={busy} onClick={() => onInstallCommand(server)} title={ui(locale, "下载 Agent", "Download Agent")}><Download size={15} /></button><button className="icon-btn" onClick={() => onEdit(server)} title={ui(locale, "编辑节点", "Edit server")}><Pencil size={15} /></button><button className="icon-btn danger" onClick={() => onRemove(server)} title={ui(locale, "删除节点", "Delete server")}><Trash2 size={15} /></button></div>
          </div>
        ))}
        {!servers.length && !busy ? <div className="list-empty">{ui(locale, "暂无节点", "No servers yet")}</div> : null}
      </div>
    </div>
  );
}
