import { RotateCw, Search, ShieldCheck, Terminal } from "lucide-react";
import type { FormEvent } from "react";
import type { AdminTab } from "../adminRoutes";
import { REMOTE_TASK_POLL_TIMEOUT_MS } from "../refresh";
import type { AdminServer, RemoteTask, TotpStatus } from "../types";
import { ui, type UiLocale } from "../locale";
import { Checkbox } from "../Checkbox";
import { remoteTaskStatusLabels } from "./shared";

export interface RemoteTabProps {
  locale: UiLocale | string | undefined;
  servers: AdminServer[];
  busy: boolean;
  selectTab: (next: AdminTab) => void;
  createRemoteTask: (event: FormEvent) => void;
  remoteCommand: string;
  setRemoteCommand: (value: string) => void;
  remoteSelectedIds: string[];
  setRemoteSelectedIds: (ids: string[]) => void;
  remoteAllSelected: boolean;
  remoteQuery: string;
  setRemoteQuery: (value: string) => void;
  remoteVisibleServers: AdminServer[];
  toggleRemoteServer: (serverId: string) => void;
  remotePayloadReady: boolean;
  remoteTasks: RemoteTask[];
  remoteTasksActive: boolean;
  remotePollingUntil: number;
  setRemotePollingUntil: (value: number) => void;
  refreshRemoteTasks: (quiet?: boolean, includeCompleted?: boolean) => void;
  remoteServerById: Map<string, AdminServer>;
  twoFactorStatus: TotpStatus | null;
}

export function RemoteTab({ locale, servers, busy, selectTab, createRemoteTask, remoteCommand, setRemoteCommand, remoteSelectedIds, setRemoteSelectedIds, remoteAllSelected, remoteQuery, setRemoteQuery, remoteVisibleServers, toggleRemoteServer, remotePayloadReady, remoteTasks, remoteTasksActive, remotePollingUntil, setRemotePollingUntil, refreshRemoteTasks, remoteServerById, twoFactorStatus }: RemoteTabProps) {
  return (
    <div className="admin-section remote-section">
      <form className="remote-execution-form" onSubmit={(event) => void createRemoteTask(event)}>
        <label className="remote-command-field">
          <span>{ui(locale, "执行命令", "Command")}</span>
          <textarea autoFocus required rows={5} maxLength={16_384} spellCheck={false} value={remoteCommand} onChange={(event) => setRemoteCommand(event.target.value)} />
          <small>{ui(locale, "多行内容按一个脚本执行，完成后返回输出，单条命令最长执行 10 分钟。离开页面不会终止已下发的命令。", "Multiple lines run as one script; output is returned when finished. A command may run up to 10 minutes. Leaving this page does not stop dispatched commands.")}</small>
        </label>

        <div className="server-picker remote-server-picker">
          <div className="server-picker-head"><strong>{ui(locale, "选择服务器", "Select servers")}</strong><span>{ui(locale, `已选 ${remoteSelectedIds.length} / 共 ${servers.length}`, `Selected ${remoteSelectedIds.length} / ${servers.length}`)}</span><button type="button" onClick={() => setRemoteSelectedIds(remoteAllSelected ? [] : servers.map((server) => server.id))}>{remoteAllSelected ? ui(locale, "取消全选", "Deselect all") : ui(locale, "全选", "Select all")}</button></div>
          <div className="server-picker-search"><Search size={16} /><input aria-label={ui(locale, "搜索远程执行服务器", "Search servers for remote execution")} placeholder={ui(locale, "搜索服务器", "Search servers")} value={remoteQuery} onChange={(event) => setRemoteQuery(event.target.value)} /></div>
          <div className="server-picker-list">
            {remoteVisibleServers.map((server) => <label className="server-picker-row" key={server.id}><Checkbox checked={remoteSelectedIds.includes(server.id)} onChange={() => toggleRemoteServer(server.id)} ariaLabel={ui(locale, `选择 ${server.name}`, `Select ${server.name}`)} /><span><strong>{server.name}</strong><small>{server.group_name || "默认"}</small></span></label>)}
            {!remoteVisibleServers.length ? <div className="server-picker-empty">{servers.length ? ui(locale, "没有匹配的服务器", "No matching servers") : ui(locale, "暂无服务器", "No servers yet")}</div> : null}
          </div>
        </div>

        {twoFactorStatus === null ? <p className="settings-hint">{ui(locale, "正在确认 TOTP 两步验证状态…", "Checking TOTP two-factor status…")}</p> : twoFactorStatus.enabled ? <div className="remote-confirm-row"><button className="primary-btn remote-submit" disabled={busy || !remoteSelectedIds.length || !remotePayloadReady}><Terminal size={15} />{busy ? ui(locale, "下发中", "Dispatching") : ui(locale, `确认执行${remoteSelectedIds.length ? ` (${remoteSelectedIds.length})` : ""}`, `Run now${remoteSelectedIds.length ? ` (${remoteSelectedIds.length})` : ""}`)}</button></div> : <div className="remote-2fa-required"><ShieldCheck size={18} /><div><strong>{ui(locale, "远程执行需要 TOTP 两步验证", "Remote execution requires TOTP two-factor authentication")}</strong><span>{ui(locale, "启用后才能向 Agent 发送命令。", "Enable it before sending commands to agents.")}</span></div><button type="button" className="secondary-btn compact" onClick={() => selectTab("security")}>{ui(locale, "前往启用", "Enable now")}</button></div>}
      </form>

      {remoteTasks.length > 0 ? <div className="remote-results">
        <div className="section-head"><div><h3>{ui(locale, "执行结果", "Results")}</h3>{remoteTasksActive ? <span className="remote-auto-refresh">{remotePollingUntil ? <RotateCw size={12} /> : null}{remotePollingUntil ? ui(locale, "等待结果，每 2 秒自动刷新", "Waiting for results; auto-refreshing every 2s") : ui(locale, "自动刷新已暂停", "Auto-refresh paused")}</span> : <span>{ui(locale, "本次命令已结束", "This run has finished")}</span>}</div><button type="button" className="secondary-btn compact" disabled={busy} onClick={() => { setRemotePollingUntil(Date.now() + REMOTE_TASK_POLL_TIMEOUT_MS); void refreshRemoteTasks(false, true); }}><RotateCw size={14} />{ui(locale, "刷新结果", "Refresh results")}</button></div>
        {remoteTasksActive && !remotePollingUntil ? <p className="settings-hint" role="status">ui(locale, "已等待 1 分钟，命令可能仍在执行。点击“刷新结果”可继续查询，暂停刷新不会停止命令。", "Waited 1 minute; the command may still be running. Click \"Refresh results\" to keep polling; pausing refresh does not stop the command.")</p> : null}
        <div className="remote-command-summary"><span>{ui(locale, "本次命令", "Command")}</span><code>{remoteTasks[0]?.command}</code></div>
        <div className="task-list">
          {remoteTasks.map((task) => {
            const server = remoteServerById.get(task.server_id);
            return <div key={task.id} className="task-item remote-result-item">
              <div className="task-header"><strong className="remote-result-server">{server?.name ?? task.server_id}</strong><span className={`task-status ${task.status}`}>{remoteTaskStatusLabels(locale)[task.status]}</span><span className="task-time">{new Date(task.requested_at * 1000).toLocaleString()}</span></div>
              {task.status === "success" || task.status === "failed" ? <pre className={`task-result ${task.status}`}>{task.result || ui(locale, "（命令没有输出）", "(no output)")}</pre> : <p className="task-progress">{task.status === "pending" ? ui(locale, "等待 Agent 确认接收；断线后不会自动重发命令。", "Waiting for the agent to acknowledge; it will not be resent after a disconnect.") : ui(locale, "Agent 已接收，正在等待执行结果…", "Received by the agent; waiting for the result…")}</p>}
            </div>;
          })}
        </div>
      </div> : null}
    </div>

  );
}
