import { useMemo, useState } from "react";
import { Copy } from "lucide-react";
import { ui } from "../../locale";
import { useDialog } from "../useDialog";
import {
  AGENT_SCRIPT_BASE,
  copyText,
  powershellLiteral,
  shellLiteral,
  type AgentInstallInfo,
  type AgentPlatform,
} from "./shared";

const PLATFORMS: Array<{ id: AgentPlatform; label: string }> = [
  { id: "linux", label: "Linux" },
  { id: "windows", label: "Windows" },
  { id: "macos", label: "macOS ARM" },
  { id: "freebsd", label: "FreeBSD" },
];

/** Builds the one-line install command for the selected platform. */
export function installCommandFor(install: AgentInstallInfo, platform: AgentPlatform, origin: string) {
  const mirror = install.agent_mirror.trim().replace(/\/+$/, "");
  const shellMirror = mirror ? ` -m ${shellLiteral(mirror)}` : "";
  const powershellMirror = mirror ? ` -Mirror ${powershellLiteral(mirror)}` : "";
  if (platform === "windows") {
    return `Invoke-WebRequest -UseBasicParsing -Uri "${AGENT_SCRIPT_BASE}/install.ps1" -OutFile "$env:TEMP\\nodeflare-install.ps1"\nUnblock-File "$env:TEMP\\nodeflare-install.ps1"\n& "$env:TEMP\\nodeflare-install.ps1" -e ${powershellLiteral(origin)} -t ${powershellLiteral(install.agent_token)}${powershellMirror}`;
  }
  if (platform === "macos") {
    return `curl -fsSL ${AGENT_SCRIPT_BASE}/install-macos.sh | sudo sh -s -- -e ${shellLiteral(origin)} -t ${shellLiteral(install.agent_token)}${shellMirror}`;
  }
  if (platform === "freebsd") {
    return `fetch -qo - ${AGENT_SCRIPT_BASE}/install-freebsd.sh | sudo sh -s -- -e ${shellLiteral(origin)} -t ${shellLiteral(install.agent_token)}${shellMirror}`;
  }
  return `curl -fsSL ${AGENT_SCRIPT_BASE}/agent.sh | sudo sh -s -- -e ${shellLiteral(origin)} -t ${shellLiteral(install.agent_token)}${shellMirror}`;
}

/** Modal that shows the per-platform Agent install command. */
export function InstallDialog({
  locale,
  install,
  onClose,
  onNotice,
  onError,
}: {
  locale: string;
  install: AgentInstallInfo;
  onClose: () => void;
  onNotice: (message: string) => void;
  onError: (message: string) => void;
}) {
  const [platform, setPlatform] = useState<AgentPlatform>("linux");
  const dialog = useDialog<HTMLElement>(true, onClose);
  const installCommand = useMemo(
    () => installCommandFor(install, platform, window.location.origin),
    [install, platform],
  );

  async function copy() {
    try {
      await copyText(installCommand);
      onNotice(ui(locale, "安装命令已复制", "Install command copied"));
    } catch {
      onError(ui(locale, "复制失败，请手动选择命令复制", "Copy failed; select and copy the command manually"));
    }
  }

  return (
    <div className="submodal-backdrop" role="presentation" onMouseDown={dialog.onBackdropMouseDown}>
      <section ref={dialog.dialogRef} className="install-modal glass-panel" role="dialog" aria-modal="true" aria-labelledby="install-dialog-title" tabIndex={-1}>
        <header><div><span className="eyebrow">{ui(locale, "Agent 部署", "Agent deployment")}</span><h3 id="install-dialog-title">{ui(locale, "下载 Agent", "Download Agent")}</h3></div><div className="segmented install-platform" role="group" aria-label={ui(locale, "Agent 平台", "Agent platform")}>{PLATFORMS.map((entry) => <button key={entry.id} type="button" className={platform === entry.id ? "active" : ""} aria-pressed={platform === entry.id} onClick={() => setPlatform(entry.id)}>{entry.label}</button>)}</div></header>
        <div className="install-list"><pre>{installCommand}</pre></div>
        <div className="form-actions"><button className="secondary-btn" type="button" onClick={onClose}>{ui(locale, "关闭", "Close")}</button><button className="primary-btn" type="button" onClick={() => void copy()}><Copy size={15} />{ui(locale, "复制", "Copy")}</button></div>
      </section>
    </div>
  );
}
