#!/bin/sh
set -eu

MONITOR_BASE_URL=${MONITOR_BASE_URL:-http://127.0.0.1:8787}
MONITOR_TURNSTILE_TOKEN=${MONITOR_TURNSTILE_TOKEN:-XXXX.DUMMY.TOKEN.XXXX}
: "${MONITOR_ADMIN_USERNAME:?Set MONITOR_ADMIN_USERNAME before running the smoke test}"
: "${MONITOR_ADMIN_PASSWORD:?Set MONITOR_ADMIN_PASSWORD before running the smoke test}"

monitor_curl() {
  case "$MONITOR_BASE_URL" in
    http://127.0.0.1:*|http://localhost:*) curl --noproxy '*' "$@" ;;
    *) curl "$@" ;;
  esac
}

request() {
  monitor_curl --fail --location --silent --show-error "$@"
}

bootstrap_json=$(request "$MONITOR_BASE_URL/api/bootstrap")
printf '%s' "$bootstrap_json" | jq -e '.access == "ok" and (.servers | type == "array")' >/dev/null
config_json=$(printf '%s' "$bootstrap_json" | jq -c '.config')
printf '%s' "$config_json" | jq -e '.site_name | length > 0' >/dev/null
password_client_salt=$(printf '%s' "$config_json" | jq -er '.password_client_salt | select(length > 0)')
password_derived=$(NODEFLARE_PASSWORD="$MONITOR_ADMIN_PASSWORD" NODEFLARE_SALT="$password_client_salt" bun -e '
const encoder = new TextEncoder();
const key = await crypto.subtle.importKey("raw", encoder.encode(process.env.NODEFLARE_PASSWORD), "PBKDF2", false, ["deriveBits"]);
const bits = await crypto.subtle.deriveBits({ name: "PBKDF2", hash: "SHA-256", iterations: 600000, salt: encoder.encode(`nodeflare:${process.env.NODEFLARE_SALT}`) }, key, 256);
console.log(Array.from(new Uint8Array(bits), byte => byte.toString(16).padStart(2, "0")).join(""));
')
cross_origin_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H 'Origin: https://not-nodeflare.invalid' "$MONITOR_BASE_URL/api/bootstrap")
[ "$cross_origin_status" = "403" ]

admin_html=$(request "$MONITOR_BASE_URL/admin")
case "$admin_html" in
  *"/admin-assets/admin.js"*) ;;
  *)
    echo "Embedded admin script is missing" >&2
    exit 1
    ;;
esac
case "$admin_html" in
  *"/admin-assets/admin.css"*) ;;
  *)
    echo "Embedded admin stylesheet is missing" >&2
    exit 1
    ;;
esac
request "$MONITOR_BASE_URL/admin-assets/admin.js" | grep -q '管理面板'
request "$MONITOR_BASE_URL/admin-assets/admin.css" | grep -q 'admin-shell'

login_json=$(request -H 'Content-Type: application/json' \
  --data "$(jq -nc --arg username "$MONITOR_ADMIN_USERNAME" --arg password "$MONITOR_ADMIN_PASSWORD" --arg password_derived "$password_derived" --arg turnstile_token "$MONITOR_TURNSTILE_TOKEN" '{username:$username,password:$password,password_derived:$password_derived,turnstile_token:$turnstile_token}')" \
  "$MONITOR_BASE_URL/api/admin/login")
admin_token=$(printf '%s' "$login_json" | jq -er '.token')

# Login protection defaults to enabled, but without a complete Turnstile pair it
# remains inactive so the first admin settings save must still work.
settings_payload=$(request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/settings" | \
  jq -c '.site_description = "Smoke settings" | del(.admin_password_configured)')
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$settings_payload" \
  "$MONITOR_BASE_URL/api/admin/settings" | jq -e '.settings.site_description == "Smoke settings"' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/bootstrap" | \
  jq -e '.config.site_description == "Smoke settings" and .exchange_rates.base == "CNY" and .exchange_rates.rates.CNY == 1 and .exchange_rates.rates.USD > 0 and .exchange_rates.rates.CAD > 0 and (.exchange_rates | has("cny") | not)' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/themes" | \
  jq -e '.themes | any(.builtin == true and .id == "builtin-nodeflare-glass" and .name == "NodeFlare Glass" and .active == true)' >/dev/null

server_input='{"name":"Smoke Test Node","region":"JP","group_name":"Test","tags":"smoke","hidden":false,"expires_at":1893456000,"traffic_limit":107374182400,"traffic_limit_type":"max","price":9.9,"billing_cycle":30,"currency":"USD","auto_renewal":true,"network_interface":"","reset_day":1,"report_interval":60,"collect_interval":5,"rx_correction":0,"tx_correction":0,"agent_mirror":"https://mirror.example.com","offline_notify_disabled":false,"auto_update":true}'

server_id=
latency_task_id=
alert_rule_id=
cleanup() {
  if [ -n "$alert_rule_id" ]; then
    monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
      -X DELETE "$MONITOR_BASE_URL/api/admin/alert-rules/$alert_rule_id" >/dev/null || true
  fi
  if [ -n "$latency_task_id" ]; then
    monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
      -X DELETE "$MONITOR_BASE_URL/api/admin/latency-tasks/$latency_task_id" >/dev/null || true
  fi
  if [ -n "$server_id" ]; then
    monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
      -X DELETE "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null || true
  fi
}
trap cleanup EXIT

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/telegram" | \
  jq -e '.telegram == null or (.telegram.bot_token == "********" and (.telegram.chat_id | length > 0))' >/dev/null
invalid_telegram_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -X PUT -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data '{"bot_token":"invalid","chat_id":"","message_thread_id":null,"template":"{{message}}"}' \
  "$MONITOR_BASE_URL/api/admin/telegram")
[ "$invalid_telegram_status" = "400" ]
latency_task_json=$(request -H "Authorization: Bearer $admin_token" \
  -H 'Content-Type: application/json' \
  --data '{"name":"Smoke TCP","task_type":"tcp","target":"example.com","port":443,"interval_seconds":60,"default_enabled":true,"server_ids":[]}' \
  "$MONITOR_BASE_URL/api/admin/latency-tasks")
latency_task_id=$(printf '%s' "$latency_task_json" | jq -er '.id')

server_json=$(request -H "Authorization: Bearer $admin_token" \
  -H 'Content-Type: application/json' \
  --data "$server_input" \
  "$MONITOR_BASE_URL/api/admin/servers")
server_id=$(printf '%s' "$server_json" | jq -er '.id')
agent_token=$(printf '%s' "$server_json" | jq -er '.agent_token')
MONITOR_BASE_URL="$MONITOR_BASE_URL" MONITOR_ADMIN_TOKEN="$admin_token" \
  MONITOR_AGENT_TOKEN="$agent_token" MONITOR_SERVER_ID="$server_id" \
  MONITOR_LATENCY_TASK_ID="$latency_task_id" \
  node scripts/websocket-smoke.mjs
token_without_admin_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id/token")
[ "$token_without_admin_status" = "401" ]
request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id/token" | \
  jq -e --arg token "$agent_token" '.agent_token == $token' >/dev/null

request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/servers" | \
  jq -e --arg id "$server_id" '.servers | any(.id == $id and .last_ip == "8.8.8.8")' >/dev/null

request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/bootstrap" | jq -e --arg id "$server_id" --arg task_id "$latency_task_id" '.servers | any(.id == $id and .cpu == 18.5 and .gpu_usage == 32.5 and .disk_await_ms == 1.4 and (.gpus | length) == 1 and (.disks | length) == 1 and .disk_used == 21474836480 and .traffic_limit == 107374182400 and .net_rx_total == 2684354560 and .net_tx_total == 1342177280 and .price == 9.9 and (has("last_ip") | not) and (.latency | any(.task_id == $task_id and .latency_ms == 48.4 and .packet_loss == 75)))' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | jq -e '.points | length >= 1 and any(.gpu_usage == 32.5)' >/dev/null
history_cache_header=$(monitor_curl --silent --dump-header - --output /dev/null \
  -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | \
  tr -d '\r' | awk -F ': ' 'tolower($1) == "x-cache" { print $2 }' | tail -1)
[ "$history_cache_header" = "HIT" ]
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | any(.id == $task_id)) and (.points | any(.task_id == $task_id and .latency_ms > 38.399 and .latency_ms < 38.401 and .packet_loss > 49.999 and .packet_loss < 50.001))' >/dev/null

# An Agent may have already measured a task when the administrator removes its
# assignment. The stale result must not block delivery of the new task list.
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data '{"name":"Smoke TCP","task_type":"tcp","target":"example.com","port":443,"interval_seconds":60,"default_enabled":true,"server_ids":[]}' \
  "$MONITOR_BASE_URL/api/admin/latency-tasks/$latency_task_id" >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | all(.id != $task_id)) and (.points | all(.task_id != $task_id))' >/dev/null
MONITOR_BASE_URL="$MONITOR_BASE_URL" MONITOR_ADMIN_TOKEN="$admin_token" \
  MONITOR_AGENT_TOKEN="$agent_token" MONITOR_SERVER_ID="$server_id" \
  MONITOR_LATENCY_TASK_ID="$latency_task_id" MONITOR_EXPECT_TASK_ASSIGNED=0 \
  MONITOR_CONFIG_ONLY=1 node scripts/websocket-smoke.mjs

alert_rule_json=$(request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' \
  --data "$(jq -nc --arg server_id "$server_id" '{name:"Smoke CPU",metric:"cpu",threshold:80,duration_minutes:5,aggregation:"average",enabled:true,server_ids:[$server_id]}')" \
  "$MONITOR_BASE_URL/api/admin/alert-rules")
alert_rule_id=$(printf '%s' "$alert_rule_json" | jq -er '.id')
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/alert-rules" | jq -e --arg id "$alert_rule_id" --arg server_id "$server_id" '.rules | any(.id == $id and .metric == "cpu" and .enabled == true and (.server_ids | index($server_id)))' >/dev/null
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$(printf '%s' "$server_input" | jq '.hidden=true')" \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/bootstrap" | \
  jq -e --arg id "$server_id" '.servers | all(.id != $id)' >/dev/null
hidden_history_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/history/$server_id?hours=1")
[ "$hidden_history_status" = "404" ]

echo "Smoke test passed"
