use std::net::IpAddr;

use worker::{D1Database, Env, Request, Response, Result};

use crate::auth::bearer_token;
use crate::db;
use crate::live;
use crate::{client_ip, error};

pub(crate) async fn websocket(req: Request, env: &Env, database: &D1Database) -> Result<Response> {
    let token = match bearer_token(&req) {
        Some(value) => value,
        None => return error("缺少 Agent Token", 401),
    };
    let Some(identity) = db::get_agent_identity(database, &token).await? else {
        return error("Agent Token 无效", 401);
    };
    if let Some(ip) = client_ip(&req).and_then(|value| value.parse::<IpAddr>().ok()) {
        db::update_last_ip(database, &identity.id, &ip.to_string()).await?;
    }
    let Some(context) = db::agent_live_context(database, &identity.id).await? else {
        return error("节点不存在", 404);
    };
    live::upgrade_agent(req, env, &identity.id, identity.hidden != 0, context).await
}
