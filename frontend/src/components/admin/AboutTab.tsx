import { ExternalLink } from "lucide-react";
import { ui, type UiLocale } from "../../locale";
import { SiteLogo } from "../SiteLogo";
import { VERSION } from "./shared";

export function AboutTab({ locale }: { locale: UiLocale | string | undefined }) {
  return (
    <div className="admin-section about-page">
      <div className="about-brand"><SiteLogo alt="" width="52" height="52" /><div><strong>NodeFlare</strong><small>{ui(locale, "基于 Rust、Axum、SQLite / PostgreSQL 与 WebSocket 的服务器监控", "Server monitoring built with Rust, Axum, SQLite / PostgreSQL and WebSocket")}</small></div></div>
      <div className="about-rows">
        <div className="about-row"><span>{ui(locale, "版本", "Version")}</span><strong>v{VERSION}</strong></div>
        <div className="about-row"><span>{ui(locale, "项目地址", "Repository")}</span><a href="https://github.com/elysia62/NodeFlare" target="_blank" rel="noreferrer">github.com/elysia62/NodeFlare<ExternalLink size={13} /></a></div>
        <div className="about-row"><span>{ui(locale, "开源协议", "License")}</span><strong>MIT License</strong></div>
      </div>
    </div>
  );
}
