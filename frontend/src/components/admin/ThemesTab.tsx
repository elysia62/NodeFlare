import { useRef, useState, type FormEvent } from "react";
import { Check, Download, ExternalLink, Eye, Trash2, Upload } from "lucide-react";
import { ui } from "../../locale";
import type { Theme } from "../../types";

type ThemeSourceMode = "repository" | "upload";

const MAX_THEME_BYTES = 32 * 1024 * 1024;

/**
 * Theme list plus the install form.
 *
 * The form owns its own draft state so the parent panel does not have to reset
 * draft fields after every install; the parent supplies the actions and reports
 * failures back through `onError`.
 */
export function ThemesTab({
  locale,
  themes,
  busy,
  onActivate,
  onPreview,
  onRemove,
  onAdd,
  onError,
}: {
  locale: string;
  themes: Theme[];
  busy: boolean;
  onActivate: (theme: Theme) => void;
  onPreview: (theme: Theme) => void;
  onRemove: (theme: Theme) => void;
  onAdd: (input: { name: string; url: string }, file: File | null) => Promise<boolean>;
  onError: (message: string) => void;
}) {
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [sourceMode, setSourceMode] = useState<ThemeSourceMode>("repository");
  const [file, setFile] = useState<File | null>(null);
  const fileInput = useRef<HTMLInputElement>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (sourceMode === "upload" && !file) {
      onError(ui(locale, "请选择 ZIP 主题文件", "Choose a ZIP theme file"));
      return;
    }
    if (sourceMode === "upload" && file && file.size > MAX_THEME_BYTES) {
      onError(ui(locale, "主题 ZIP 不能超过 32 MiB", "Theme ZIP must not exceed 32 MiB"));
      return;
    }
    const installed = await onAdd(
      { name: name.trim(), url: url.trim() },
      sourceMode === "upload" ? file : null,
    );
    if (!installed) return;
    setName(""); setUrl(""); setFile(null);
    if (fileInput.current) fileInput.current.value = "";
  }

  return (
    <div className="theme-store-page">
      <section className="admin-section">
        <div className="section-head"><h3>{ui(locale, "主题列表", "Themes")}</h3></div>
        <div className="theme-list">
          {themes.map((theme) => {
            const uploaded = theme.url.startsWith("upload:");
            return <article className={`theme-row ${theme.active ? "active" : ""}`} key={theme.id}>
              <div className="theme-row-main">
                <div className="theme-row-title"><strong>{theme.name}</strong><span className={`theme-badge ${theme.builtin ? "builtin" : uploaded ? "upload" : "remote"}`}>{theme.builtin ? ui(locale, "默认主题", "Built-in") : uploaded ? ui(locale, "上传安装", "Uploaded") : "GitHub Release"}</span>{theme.version ? <span className="theme-badge version">v{theme.version}</span> : null}</div>
                {!theme.builtin && uploaded ? <span className="theme-upload-source"><Upload size={13} /><span>{theme.url.slice("upload:".length)}</span></span> : null}
                {!theme.builtin && !uploaded ? <a href={theme.url} target="_blank" rel="noreferrer"><span>{theme.url}</span><ExternalLink size={13} /></a> : null}
              </div>
              <div className="theme-row-actions">
                {!theme.builtin ? <button type="button" className="secondary-btn compact" disabled={busy} onClick={() => onPreview(theme)}><Eye size={15} />{ui(locale, "预览", "Preview")}</button> : null}
                <button type="button" className={theme.active ? "theme-active-btn" : "primary-btn compact"} disabled={busy || theme.active} onClick={() => onActivate(theme)}>{theme.active ? <><Check size={15} />{ui(locale, "使用中", "Active")}</> : ui(locale, "启用", "Activate")}</button>
                {!theme.builtin ? <button type="button" className="icon-btn danger" disabled={busy} title={ui(locale, "删除主题", "Delete theme")} onClick={() => onRemove(theme)}><Trash2 size={15} /></button> : null}
              </div>
            </article>;
          })}
        </div>
      </section>
      <form className="admin-section theme-add-form" onSubmit={submit}>
        <div className="section-head"><div><h3>{ui(locale, "安装主题", "Install theme")}</h3><span>{ui(locale, "主题包含可执行前端代码，只安装可信来源；安装后不依赖运行时远程资源。", "Themes contain executable frontend code; install only from trusted sources. No runtime remote resources are needed after installation.")}</span></div></div>
        <div className="segmented theme-source-tabs" role="group" aria-label={ui(locale, "主题安装来源", "Theme install source")}>
          <button type="button" className={sourceMode === "repository" ? "active" : ""} aria-pressed={sourceMode === "repository"} onClick={() => setSourceMode("repository")}>{ui(locale, "GitHub 仓库", "GitHub repository")}</button>
          <button type="button" className={sourceMode === "upload" ? "active" : ""} aria-pressed={sourceMode === "upload"} onClick={() => setSourceMode("upload")}>{ui(locale, "上传", "Upload")}</button>
        </div>
        <div className="form-grid"><label><span>{ui(locale, "主题名称", "Theme name")}</span><input required maxLength={80} value={name} onChange={(event) => setName(event.target.value)} placeholder={ui(locale, "例如：Ocean", "e.g. Ocean")} /></label>{sourceMode === "repository" ? <label><span>{ui(locale, "GitHub 仓库", "GitHub repository")}</span><input required type="url" maxLength={2048} value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://github.com/user/theme" /></label> : <label className="theme-file-field"><span>{ui(locale, "文件", "File")}</span><input ref={fileInput} required type="file" accept=".zip,application/zip" onChange={(event) => setFile(event.target.files?.[0] ?? null)} />{file ? <small>{`${file.name} · ${(file.size / 1024 / 1024).toFixed(2)} MiB`}</small> : null}</label>}</div>
        <div className="form-actions"><button className="primary-btn" disabled={busy || !name.trim() || (sourceMode === "repository" ? !url.trim() : !file)}>{sourceMode === "upload" ? <Upload size={15} /> : <Download size={15} />}{busy ? ui(locale, "安装中", "Installing") : ui(locale, "安装主题", "Install theme")}</button></div>
      </form>
    </div>
  );
}
