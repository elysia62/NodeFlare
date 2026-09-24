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
 * five fields after every install; the parent supplies the actions and reports
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
  onAdd: (input: { name: string; description: string; url: string }, file: File | null) => Promise<boolean>;
  onError: (message: string) => void;
}) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
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
      { name: name.trim(), description: description.trim(), url: url.trim() },
      sourceMode === "upload" ? file : null,
    );
    if (!installed) return;
    setName(""); setDescription(""); setUrl(""); setFile(null);
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
                {!theme.builtin ? <p title={theme.description || undefined}>{theme.description || ui(locale, "暂无主题说明", "No description")}</p> : null}
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
        <p className="settings-hint">{sourceMode === "repository" ? ui(locale, "填写仓库主页地址，NodeFlare 会下载 latest Release 中的第一个 ZIP 文件。", "Enter the repository URL; NodeFlare downloads the first ZIP asset of the latest release.") : ui(locale, "ZIP 根目录需包含 index.html，也支持外层只有一个目录的打包方式；最大 32 MiB。", "The ZIP must contain index.html at its root (a single wrapping directory is fine); max 32 MiB.")}</p>
        <div className="form-grid"><label><span>{ui(locale, "主题名称", "Theme name")}</span><input required maxLength={80} value={name} onChange={(event) => setName(event.target.value)} placeholder={ui(locale, "例如：Ocean", "e.g. Ocean")} /></label>{sourceMode === "repository" ? <label><span>{ui(locale, "GitHub 仓库", "GitHub repository")}</span><input required type="url" maxLength={2048} value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://github.com/user/theme" /></label> : <label className="theme-file-field"><span>{ui(locale, "文件", "File")}</span><input ref={fileInput} required type="file" accept=".zip,application/zip" onChange={(event) => setFile(event.target.files?.[0] ?? null)} /><small>{file ? `${file.name} · ${(file.size / 1024 / 1024).toFixed(2)} MiB` : ui(locale, "请选择 .zip 文件", "Choose a .zip file")}</small></label>}</div>
        <label><span>{ui(locale, "主题说明（可选）", "Theme description (optional)")}</span><textarea rows={2} maxLength={300} value={description} onChange={(event) => setDescription(event.target.value)} placeholder={ui(locale, "简短描述主题风格和来源", "Briefly describe the style and source")} /></label>
        <div className="form-actions"><button className="primary-btn" disabled={busy || (sourceMode === "upload" && !file)}>{sourceMode === "upload" ? <Upload size={15} /> : <Download size={15} />}{busy ? ui(locale, "安装中", "Installing") : ui(locale, "安装主题", "Install theme")}</button></div>
      </form>
    </div>
  );
}
