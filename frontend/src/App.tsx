import { Megaphone, Moon, Search, Sun, UserCircle } from "lucide-react";
import { lazy, Suspense, useEffect, useMemo, useState } from "react";
import { demoMode, useApp } from "./AppProvider";
import { LoginForm } from "./components/LoginForm";
import { NodeCard } from "./components/NodeCard";
import { SiteLogo } from "./components/SiteLogo";
import { StatsBar } from "./components/StatsBar";
import { TurnstileWidget } from "./components/TurnstileWidget";
import { ui } from "./locale";

const NodeDetails = lazy(() => import("./components/NodeDetails").then((module) => ({ default: module.NodeDetails })));

function LoginGate() {
  const { config, dark, error, setError, login } = useApp();
  const locale = config.locale;

  return (
    <section className="dashboard-login-gate glass-panel">
      <SiteLogo src={config.logo_url} alt="" width="46" height="46" />
      <div className="dashboard-login-copy">
        <h1>{ui(locale, "登录仪表盘", "Sign in to dashboard")}</h1>
        <p>{ui(locale, "此仪表盘仅限登录后访问", "This dashboard requires an administrator sign-in")}</p>
      </div>
      <LoginForm
        config={config}
        dark={dark}
        error={error}
        setError={setError}
        onSubmit={login}
        className="dashboard-login-form"
        turnstileClassName="dashboard-login-turnstile"
        errorClassName="form-error"
        submitClassName="primary-btn dashboard-login-submit"
      />
    </section>
  );
}

function VerificationGate() {
  const { config, dark, error, setError, verify } = useApp();
  const [busy, setBusy] = useState(false);
  const [resetKey, setResetKey] = useState(0);
  const locale = config.locale;

  async function run(token: string) {
    if (!token || busy) return;
    setBusy(true);
    setError("");
    try {
      await verify(token);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : ui(locale, "Cloudflare 验证失败", "Cloudflare verification failed"));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="verification-gate">
      <SiteLogo src={config.logo_url} alt="" width="52" height="52" />
      <div><h1>{ui(locale, "访问验证", "Access verification")}</h1><p>{config.site_name}</p></div>
      {config.turnstile_site_key
        ? <TurnstileWidget siteKey={config.turnstile_site_key} action="public_dashboard" theme={dark ? "dark" : "light"} resetKey={resetKey} onVerify={(token) => void run(token)} onError={setError} locale={locale} />
        : <p className="form-error">{ui(locale, "Turnstile 尚未正确配置", "Turnstile is not configured")}</p>}
      {busy ? <span className="verification-status">{ui(locale, "正在验证", "Verifying")}</span> : null}
      {error ? <p className="form-error">{error}</p> : null}
      {error && !busy ? <button className="secondary-btn" type="button" onClick={() => { setError(""); setResetKey((value) => value + 1); }}>{ui(locale, "重试", "Retry")}</button> : null}
    </section>
  );
}

function HomeView() {
  const { carrierLatency, config, error, exchangeRates, liveConnected, liveMetrics, loading, openServer, reload, servers } = useApp();
  const [query, setQuery] = useState("");
  const [group, setGroup] = useState("__all__");
  const locale = config.locale;
  const fallbackGroup = ui(locale, "默认", "Default");

  const groups = useMemo(
    () => ["__all__", ...Array.from(new Set(servers.map((server) => server.group_name || fallbackGroup)))],
    [fallbackGroup, servers],
  );
  const visible = useMemo(() => servers.filter((server) => {
    const text = `${server.name} ${server.region} ${server.tags} ${server.group_name}`.toLowerCase();
    const groupMatches = !config.show_groups || group === "__all__" || (server.group_name || fallbackGroup) === group;
    const queryMatches = !config.show_search || text.includes(query.trim().toLowerCase());
    return groupMatches && queryMatches;
  }), [config.show_groups, config.show_search, fallbackGroup, group, query, servers]);

  // A locale change renames the fallback group; drop the stale selection.
  useEffect(() => setGroup("__all__"), [fallbackGroup]);

  return (
    <div className="home-content">
      <StatsBar servers={servers} config={config} exchangeRates={exchangeRates} />
      {config.show_search || config.show_groups ? <div className="toolbar">
        {config.show_search ? <div className="search-box"><Search size={16} /><input aria-label={ui(locale, "搜索节点", "Search servers")} placeholder={ui(locale, "搜索节点", "Search servers")} value={query} onChange={(event) => setQuery(event.target.value)} /></div> : null}
        {config.show_groups ? <div className="group-tabs" role="group" aria-label={ui(locale, "节点分组", "Server groups")}>{groups.map((item) => <button className={group === item ? "active" : ""} aria-pressed={group === item} key={item} onClick={() => setGroup(item)}>{item === "__all__" ? ui(locale, "全部", "All") : item}</button>)}</div> : null}
        <span className="result-count">{ui(locale, `${visible.length} 个节点`, `${visible.length} servers`)}</span>
      </div> : null}
      {error ? <div className="error-band"><span>{error}</span><button onClick={() => void reload()}>{ui(locale, "重试", "Retry")}</button></div> : null}
      {loading && !servers.length ? (
        <div className="dashboard-loading"><span className="loading-ring" aria-hidden="true" /><p>{ui(locale, "加载中…", "Loading…")}</p></div>
      ) : visible.length ? (
        <section className={`node-grid ${carrierLatency ? "carrier-latency" : ""}`}>
          {visible.map((server) => <NodeCard key={server.id} server={server} config={config} liveConnected={liveConnected} liveLatencyResults={liveMetrics[server.id]?.latencyResults} onOpen={openServer} />)}
        </section>
      ) : !error ? (
        <div className="empty-state"><strong>{servers.length ? ui(locale, "没有匹配的节点", "No matching servers") : ui(locale, "尚未添加节点", "No servers added")}</strong></div>
      ) : null}
    </div>
  );
}

function DetailView() {
  const { config, goHome, liveMetrics, loading, selectedId, servers } = useApp();
  const locale = config.locale;
  const selected = servers.find((server) => server.id === selectedId) ?? null;

  if (selected) {
    return (
      <Suspense fallback={<div className="chart-loading">{ui(locale, "正在加载节点", "Loading server")}</div>}>
        <NodeDetails
          key={selected.id}
          server={selected}
          liveLatencyResults={liveMetrics[selected.id]?.latencyResults}
          threshold={config.offline_threshold_seconds}
          retentionDays={config.history_retention_days}
          locale={locale}
          demo={demoMode}
          onClose={goHome}
        />
      </Suspense>
    );
  }
  if (loading) return <div className="chart-loading">{ui(locale, "正在加载节点", "Loading server")}</div>;
  return (
    <div className="empty-state">
      <strong>{ui(locale, "节点不存在或已隐藏", "Server not found or hidden")}</strong>
      <button className="primary-btn" onClick={goHome}>{ui(locale, "返回首页", "Back")}</button>
    </div>
  );
}

export default function App() {
  const { access, background, config, configReady, dark, error, loading, reload, selectedId, toggleTheme } = useApp();
  const locale = config.locale;

  if (!configReady) {
    return (
      <div className="app-bootstrap" aria-busy={loading}>
        {error ? <div className="error-band"><span>{error}</span><button onClick={() => void reload()}>{ui(locale, "重试", "Retry")}</button></div> : null}
      </div>
    );
  }

  return (
    <div className="app-shell">
      <div className="theme-background" style={background ? { backgroundImage: `url(${JSON.stringify(background)})`, opacity: 1 } : undefined} />
      <header className="site-header">
        <div className="container header-inner">
          <div className="brand"><SiteLogo src={config.logo_url} alt="" width="36" height="36" /><strong>{config.site_name}</strong></div>
          <div className="header-actions">
            <button className="icon-btn" onClick={toggleTheme} title={dark ? ui(locale, "浅色主题", "Light theme") : ui(locale, "深色主题", "Dark theme")}>{dark ? <Sun size={18} /> : <Moon size={18} />}</button>
            <a className="icon-btn" href="/admin/login" target="_blank" rel="noopener noreferrer" title={ui(locale, "进入后台", "Administration")} aria-label={ui(locale, "进入后台", "Administration")}><UserCircle size={18} /></a>
          </div>
        </div>
      </header>

      <main className="container main-content">
        {config.site_announcement ? <div className="site-announcement"><Megaphone size={16} /><span>{config.site_announcement}</span></div> : null}
        {access === "login" ? <LoginGate />
          : access === "turnstile" ? <VerificationGate />
            : selectedId ? <DetailView />
              : <HomeView />}
      </main>
    </div>
  );
}
