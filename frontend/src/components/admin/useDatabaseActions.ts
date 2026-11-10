import { useState, type Dispatch, type SetStateAction } from "react";
import { api, type SensitiveProof } from "../../api";
import { formatBytes } from "../../format";
import { ui } from "../../locale";
import type { DatabaseMigrationResult, DatabaseStats, ExchangeRates } from "../../types";
import { waitForDatabaseSwitch } from "./shared";

type Feedback = Dispatch<SetStateAction<string>>;

export function useDatabaseActions({ locale, setBusy, setError, setNotice, sensitiveProof }: {
  locale: string;
  setBusy: Dispatch<SetStateAction<boolean>>;
  setError: Feedback;
  setNotice: Feedback;
  sensitiveProof: (action: string) => Promise<SensitiveProof | null>;
}) {
  const [database, setDatabase] = useState<DatabaseStats | null>(null);
  const [databaseMigrationUrl, setDatabaseMigrationUrl] = useState("");
  const [databaseMigrationResult, setDatabaseMigrationResult] = useState<DatabaseMigrationResult | null>(null);
  const [restarting, setRestarting] = useState(false);
  const [exchangeRates, setExchangeRates] = useState<ExchangeRates | null>(null);

  async function loadDatabase() {
    setBusy(true); setError("");
    try {
      const [stats, rates] = await Promise.all([api.databaseStats(), api.exchangeRates()]);
      setDatabase(stats);
      if (!stats.restart_required) setDatabaseMigrationResult(null);
      setExchangeRates(rates);
    }
    catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "读取数据库统计失败", "Failed to load database stats")); }
    finally { setBusy(false); }
  }

  async function reclaimDatabase() {
    if (!window.confirm(ui(locale, "回收空间期间数据库会短暂不可用，确认继续？", "The database will be briefly unavailable while reclaiming space. Continue?"))) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const result = await api.reclaimDatabase();
      setDatabase(result.database);
      setNotice(result.reclaimed_bytes > 0 ? ui(locale, `已回收 ${formatBytes(result.reclaimed_bytes)}`, `Reclaimed ${formatBytes(result.reclaimed_bytes)}`) : ui(locale, "数据库已整理，当前没有可释放空间", "Database optimized; nothing to reclaim right now"));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "回收数据库空间失败", "Failed to reclaim database space")); }
    finally { setBusy(false); }
  }

  async function migrateDatabase() {
    const databaseUrl = databaseMigrationUrl.trim();
    if (!databaseUrl) return;
    const source = database?.kind === "postgresql" ? "PostgreSQL" : "SQLite";
    const target = database?.kind === "postgresql" ? "SQLite" : "PostgreSQL";
    if (!window.confirm(ui(locale, `将 ${source} 迁移到 ${target}？目标库中已有的 NodeFlare 数据会被覆盖，完成后业务写入将暂停，直至重启服务。`, `Migrate from ${source} to ${target}? Existing NodeFlare data in the target database will be overwritten, and writes will pause until the service restarts.`))) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof(ui(locale, "迁移数据库", "Migrate database"));
      if (!proof) return;
      const result = await api.migrateDatabase(databaseUrl, proof);
      setDatabaseMigrationUrl("");
      setDatabaseMigrationResult(result);
      setDatabase((current) => current ? { ...current, restart_required: result.restart_required } : current);
      const targetName = result.target_kind === "postgresql" ? "PostgreSQL" : "SQLite";
      setNotice(ui(locale, `数据库已迁移到 ${targetName}，共 ${result.migrated_rows.toLocaleString()} 行（${formatBytes(result.size_bytes)}），业务写入已暂停，请重启服务`, `Migrated to ${targetName}: ${result.migrated_rows.toLocaleString()} rows (${formatBytes(result.size_bytes)}). Writes are paused; restart the service`));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "数据库迁移失败", "Database migration failed")); }
    finally { setBusy(false); }
  }

  async function restartAfterDatabaseMigration() {
    if (!database?.restart_required || !window.confirm(ui(locale, "立即重启 NodeFlare 并切换到新数据库？管理界面和 Agent 会短暂断开。", "Restart NodeFlare now and switch to the new database? The admin UI and agents will disconnect briefly."))) return;
    const targetKind = databaseMigrationResult?.target_kind === "postgresql"
      ? "postgresql"
      : databaseMigrationResult?.target_kind === "sqlite"
        ? "sqlite"
        : database.kind === "postgresql" ? "sqlite" : "postgresql";
    setBusy(true); setRestarting(true); setError(""); setNotice(ui(locale, "NodeFlare 正在重启", "NodeFlare is restarting"));
    try {
      await api.restartAfterDatabaseMigration();
      await waitForDatabaseSwitch(targetKind, locale);
      window.location.replace("/admin/login");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(locale, "重启 NodeFlare 失败", "Failed to restart NodeFlare"));
    } finally {
      setBusy(false);
      setRestarting(false);
    }
  }

  async function refreshExchangeRates() {
    setBusy(true); setError(""); setNotice("");
    try {
      const rates = await api.refreshExchangeRates();
      setExchangeRates(rates);
      setNotice(ui(locale, `汇率已更新 · ${rates.source} · ${rates.date}`, `Exchange rates updated · ${rates.source} · ${rates.date}`));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "汇率更新失败", "Failed to refresh exchange rates")); }
    finally { setBusy(false); }
  }

  async function exportDatabaseBackup() {
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof(ui(locale, "导出备份", "Export backup"));
      if (!proof) return;
      const { blob, filename } = await api.databaseBackup(proof);
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = filename;
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      setNotice(ui(locale, "数据库备份已导出", "Database backup exported"));
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "导出数据库备份失败", "Failed to export the database backup")); }
    finally { setBusy(false); }
  }

  async function restoreDatabaseBackup(file: File) {
    if (file.size > 512 * 1024 * 1024) {
      setError(ui(locale, "数据库备份 ZIP 不能超过 512 MiB", "Database backup ZIP must not exceed 512 MiB"));
      return;
    }
    if (!window.confirm(ui(locale, "恢复将覆盖当前数据库，并使现有登录会话失效。确认继续？", "Restoring overwrites the current database and invalidates existing sessions. Continue?"))) return;
    setBusy(true); setError(""); setNotice("");
    try {
      const proof = await sensitiveProof(ui(locale, "恢复备份", "Restore backup"));
      if (!proof) return;
      const result = await api.restoreDatabaseBackup(file, proof);
      window.alert(ui(locale, `数据库已恢复 ${result.restored_rows.toLocaleString()} 行，请使用备份中的管理员账号重新登录。`, `Restored ${result.restored_rows.toLocaleString()} rows. Sign in again with the admin account from the backup.`));
      window.location.reload();
    } catch (reason) { setError(reason instanceof Error ? reason.message : ui(locale, "恢复数据库备份失败", "Failed to restore the database backup")); }
    finally { setBusy(false); }
  }

  return {
    database, databaseMigrationUrl, setDatabaseMigrationUrl, databaseMigrationResult,
    restarting, exchangeRates, loadDatabase, reclaimDatabase, migrateDatabase,
    restartAfterDatabaseMigration, refreshExchangeRates, exportDatabaseBackup, restoreDatabaseBackup,
  };
}
