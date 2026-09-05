#!/bin/sh
set -eu

MONITOR_BASE_URL=${MONITOR_BASE_URL:-http://127.0.0.1:8080}
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

step() {
  printf 'smoke: %s\n' "$1" >&2
}

admin_token=
server_id=
latency_task_id=
agent_latency_task_id=
alert_rule_id=
backup_file=
agent_pid=
agent_state_dir=
cleanup() {
  if [ -n "$agent_pid" ]; then
    kill "$agent_pid" 2>/dev/null || true
    wait "$agent_pid" 2>/dev/null || true
  fi
  [ -z "$agent_state_dir" ] || rm -rf "$agent_state_dir"
  [ -z "$backup_file" ] || rm -f -- "$backup_file"
  if [ "${MONITOR_KEEP_RESOURCES:-0}" != "1" ] && [ -n "$admin_token" ]; then
    if [ -n "$alert_rule_id" ]; then
      monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
        -X DELETE "$MONITOR_BASE_URL/api/admin/alert-rules/$alert_rule_id" >/dev/null || true
    fi
    if [ -n "$latency_task_id" ]; then
      monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
        -X DELETE "$MONITOR_BASE_URL/api/admin/latency-tasks/$latency_task_id" >/dev/null || true
    fi
    if [ -n "$agent_latency_task_id" ]; then
      monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
        -X DELETE "$MONITOR_BASE_URL/api/admin/latency-tasks/$agent_latency_task_id" >/dev/null || true
    fi
    if [ -n "$server_id" ]; then
      monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
        -X DELETE "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null || true
    fi
  fi
  if [ -n "$admin_token" ]; then
    monitor_curl --silent --show-error -H "Authorization: Bearer $admin_token" \
      -X POST "$MONITOR_BASE_URL/api/admin/logout" >/dev/null || true
  fi
}
trap cleanup EXIT

step "bootstrap"
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
step "login throttling"
invalid_login_payload=$(jq -nc --arg username "$MONITOR_ADMIN_USERNAME" --arg password_derived "0000000000000000000000000000000000000000000000000000000000000000" '{username:$username,password_derived:$password_derived,turnstile_token:""}')
attempt=1
while [ "$attempt" -le 5 ]; do
  invalid_login_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
    -H 'Content-Type: application/json' -H 'X-Forwarded-For: 203.0.113.10' \
    --data "$invalid_login_payload" "$MONITOR_BASE_URL/api/admin/login")
  [ "$invalid_login_status" = "401" ]
  attempt=$((attempt + 1))
done
throttled_login_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H 'Content-Type: application/json' -H 'X-Forwarded-For: 203.0.113.10' \
  --data "$invalid_login_payload" "$MONITOR_BASE_URL/api/admin/login")
[ "$throttled_login_status" = "429" ]
step "static assets"
security_headers=$(monitor_curl --silent --show-error --dump-header - --output /dev/null "$MONITOR_BASE_URL/")
printf '%s' "$security_headers" | grep -qi '^x-content-type-options: nosniff'
printf '%s' "$security_headers" | grep -qi '^x-frame-options: DENY'
printf '%s' "$security_headers" | grep -qi "^content-security-policy:.*frame-ancestors 'none'"
admin_html=$(request "$MONITOR_BASE_URL/admin/login")
request "$MONITOR_BASE_URL/admin/servers" | grep -q '/admin-assets/'
request "$MONITOR_BASE_URL/admin/about" | grep -q '/admin-assets/'
admin_script=$(printf '%s' "$admin_html" | sed -n 's/.*src="\([^"]*\.js\)".*/\1/p' | head -n 1)
admin_stylesheet=$(printf '%s' "$admin_html" | sed -n 's/.*href="\([^"]*\.css\)".*/\1/p' | head -n 1)
[ -n "$admin_script" ] || { echo "Admin script is missing" >&2; exit 1; }
[ -n "$admin_stylesheet" ] || { echo "Admin stylesheet is missing" >&2; exit 1; }
request "$MONITOR_BASE_URL$admin_script" | grep -q '管理面板'
request "$MONITOR_BASE_URL$admin_stylesheet" | grep -q 'admin-shell'
admin_headers=$(monitor_curl --silent --show-error --dump-header - --output /dev/null "$MONITOR_BASE_URL/admin")
printf '%s' "$admin_headers" | grep -qi '^cache-control:.*no-store'

step "login and settings"
login_payload=$(jq -nc --arg username "$MONITOR_ADMIN_USERNAME" --arg password_derived "$password_derived" --arg turnstile_token "$MONITOR_TURNSTILE_TOKEN" '{username:$username,password_derived:$password_derived,turnstile_token:$turnstile_token}')
login_admin() {
  request -H 'Content-Type: application/json' --data "$login_payload" \
    "$MONITOR_BASE_URL/api/admin/login"
}
login_json=$(login_admin)
admin_token=$(printf '%s' "$login_json" | jq -er '.token')
api_headers=$(monitor_curl --silent --show-error --dump-header - --output /dev/null \
  -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/settings")
printf '%s' "$api_headers" | grep -qi '^cache-control:.*no-store'

step "login devices"
second_login_json=$(login_admin)
second_admin_token=$(printf '%s' "$second_login_json" | jq -er '.token')
sessions_json=$(request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/sessions")
printf '%s' "$sessions_json" | jq -e '
  (.sessions | length) >= 2 and
  ([.sessions[] | select(.current == true)] | length) == 1 and
  ([.sessions[] | select(.current == false)] | length) >= 1 and
  (.sessions | all((.ip_address | length) > 0 and (.user_agent | length) > 0))
' >/dev/null
second_session_id=$(request -H "Authorization: Bearer $second_admin_token" \
  "$MONITOR_BASE_URL/api/admin/sessions" | jq -er '.sessions[] | select(.current == true) | .id')
request -H "Authorization: Bearer $admin_token" -X DELETE \
  "$MONITOR_BASE_URL/api/admin/sessions/$second_session_id" >/dev/null
revoked_session_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $second_admin_token" "$MONITOR_BASE_URL/api/admin/settings")
[ "$revoked_session_status" = "401" ]

settings_payload=$(request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/settings" | \
  jq -c '.site_description = "Smoke settings" | del(.admin_password_configured, .totp_login_enabled)')
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$settings_payload" \
  "$MONITOR_BASE_URL/api/admin/settings" | jq -e '.site_description == "Smoke settings"' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/bootstrap" | \
  jq -e '.config.site_description == "Smoke settings" and .exchange_rates.base == "CNY" and .exchange_rates.rates.CNY == 1 and .exchange_rates.rates.USD > 0 and .exchange_rates.rates.CAD > 0 and (.exchange_rates | has("cny") | not)' >/dev/null

request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/themes" | \
  jq -e '.themes | any(.builtin == true and .id == "builtin-nodeflare-glass" and .name == "NodeFlare Glass" and .active == true)' >/dev/null

step "database size and reclaim"
request -H "Authorization: Bearer $admin_token" \
  "$MONITOR_BASE_URL/api/admin/database" | \
  jq -e '(.kind == "sqlite" or .kind == "postgresql") and .size_bytes > 0' >/dev/null
request -H "Authorization: Bearer $admin_token" -X POST \
  "$MONITOR_BASE_URL/api/admin/database/reclaim" | \
  jq -e '.database.size_bytes > 0 and .reclaimed_bytes >= 0' >/dev/null

step "database backup and restore"
backup_file=$(mktemp /tmp/nodeflare-backup.XXXXXX.zip)
request -H "Authorization: Bearer $admin_token" -H "X-NodeFlare-Password: $password_derived" \
  "$MONITOR_BASE_URL/api/admin/database/backup" > "$backup_file"
[ -s "$backup_file" ]
changed_settings=$(printf '%s' "$settings_payload" | jq -c '.site_description = "Changed after backup"')
request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data "$changed_settings" "$MONITOR_BASE_URL/api/admin/settings" >/dev/null
restore_json=$(request -H "Authorization: Bearer $admin_token" \
  -H "X-NodeFlare-Password: $password_derived" -H 'Content-Type: application/zip' \
  --data-binary "@$backup_file" "$MONITOR_BASE_URL/api/admin/database/restore?filename=smoke.zip")
printf '%s' "$restore_json" | jq -e '.restored_rows > 0' >/dev/null
restored_session_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/settings")
[ "$restored_session_status" = "401" ]
admin_token=$(login_admin | jq -er '.token')
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/bootstrap" | \
  jq -e '.config.site_description == "Smoke settings"' >/dev/null
rm -f -- "$backup_file"
backup_file=

server_input='{"name":"Smoke Test Node","region":"JP","group_name":"Test","tags":"smoke","hidden":false,"expires_at":1893456000,"traffic_limit":107374182400,"traffic_limit_type":"max","price":9.9,"billing_cycle":30,"currency":"USD","auto_renewal":true,"network_interface":"","reset_day":1,"report_interval":60,"collect_interval":5,"rx_correction":0,"tx_correction":0,"agent_mirror":"https://mirror.example.com","offline_notify_disabled":false,"auto_update":true}'

step "admin resources"
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
step "websocket report"
MONITOR_BASE_URL="$MONITOR_BASE_URL" MONITOR_ADMIN_TOKEN="$admin_token" \
  MONITOR_ADMIN_USERNAME="$MONITOR_ADMIN_USERNAME" \
  MONITOR_PASSWORD_DERIVED="$password_derived" \
  MONITOR_AGENT_TOKEN="$agent_token" MONITOR_SERVER_ID="$server_id" \
  MONITOR_LATENCY_TASK_ID="$latency_task_id" \
  node scripts/websocket-smoke.mjs
token_without_admin_status=$(monitor_curl --silent --output /dev/null --write-out '%{http_code}' \
  -X POST \
  "$MONITOR_BASE_URL/api/admin/servers/$server_id/token")
[ "$token_without_admin_status" = "401" ]
rotated_agent_token=$(MONITOR_BASE_URL="$MONITOR_BASE_URL" MONITOR_ADMIN_TOKEN="$admin_token" \
  MONITOR_AGENT_TOKEN="$agent_token" MONITOR_SERVER_ID="$server_id" \
  MONITOR_LATENCY_TASK_ID="$latency_task_id" MONITOR_CONFIG_ONLY=1 \
  MONITOR_ROTATE_AGENT_TOKEN=1 node scripts/websocket-smoke.mjs)
[ "$rotated_agent_token" != "$agent_token" ]
agent_token=$rotated_agent_token

step "persisted metrics"
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/admin/servers" | \
  jq -e --arg id "$server_id" '.servers | any(.id == $id and .last_ip == "8.8.8.8")' >/dev/null

post_report_bootstrap=$(request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/bootstrap")
if ! printf '%s' "$post_report_bootstrap" | jq -e --arg id "$server_id" --arg task_id "$latency_task_id" '.servers | any(.id == $id and .cpu == 18.5 and .gpu_usage == 32.5 and .disk_await_ms == 1.4 and (.gpus | length) == 1 and (.disks | length) == 1 and .disk_used == 21474836480 and .traffic_limit == 107374182400 and .net_rx_total == 2684354560 and .net_tx_total == 1342177280 and .price == 9.9 and (has("last_ip") | not) and (.latency | any(.task_id == $task_id and .latency_ms == 48.4 and .packet_loss == 75)))' >/dev/null; then
  printf '%s' "$post_report_bootstrap" | jq --arg id "$server_id" '.servers[] | select(.id == $id)' >&2
  exit 1
fi
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/history/$server_id?hours=1" | jq -e '.points | length >= 1 and any(.gpu_usage == 32.5)' >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | any(.id == $task_id)) and (.points | any(.task_id == $task_id and .latency_ms > 38.399 and .latency_ms < 38.401 and .packet_loss > 49.999 and .packet_loss < 50.001))' >/dev/null

request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
  --data '{"name":"Smoke TCP","task_type":"tcp","target":"example.com","port":443,"interval_seconds":60,"default_enabled":true,"server_ids":[]}' \
  "$MONITOR_BASE_URL/api/admin/latency-tasks/$latency_task_id" >/dev/null
request -H "Authorization: Bearer $admin_token" "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | jq -e --arg task_id "$latency_task_id" '(.tasks | all(.id != $task_id)) and (.points | all(.task_id != $task_id))' >/dev/null
step "agent reconfiguration"
MONITOR_BASE_URL="$MONITOR_BASE_URL" MONITOR_ADMIN_TOKEN="$admin_token" \
  MONITOR_AGENT_TOKEN="$agent_token" MONITOR_SERVER_ID="$server_id" \
  MONITOR_LATENCY_TASK_ID="$latency_task_id" MONITOR_EXPECT_TASK_ASSIGNED=0 \
  MONITOR_CONFIG_ONLY=1 node scripts/websocket-smoke.mjs

if [ -n "${MONITOR_AGENT_BINARY:-}" ]; then
  step "real Agent failed-latency and persistence"
  request -H "Authorization: Bearer $admin_token" -H 'Content-Type: application/json' -X PATCH \
    --data "$(printf '%s' "$server_input" | jq '.report_interval=15 | .collect_interval=1')" \
    "$MONITOR_BASE_URL/api/admin/servers/$server_id" >/dev/null
  agent_latency_task_id=$(request -H "Authorization: Bearer $admin_token" \
    -H 'Content-Type: application/json' \
    --data "$(jq -nc --arg server_id "$server_id" '{name:"Smoke failure",task_type:"tcp",target:"does-not-exist.invalid",port:443,interval_seconds:30,default_enabled:false,server_ids:[$server_id]}')" \
    "$MONITOR_BASE_URL/api/admin/latency-tasks" | jq -er '.id')
  agent_state_dir=$(mktemp -d "${TMPDIR:-/tmp}/nodeflare-agent-smoke.XXXXXX")
  NODEFLARE_STATE_DIR="$agent_state_dir" "$MONITOR_AGENT_BINARY" \
    -e "$MONITOR_BASE_URL" -t "$agent_token" -i 15 > "$agent_state_dir/agent.log" 2>&1 &
  agent_pid=$!
  failed_latency_seen=false
  attempt=1
  while [ "$attempt" -le 45 ]; do
    if request -H "Authorization: Bearer $admin_token" \
      "$MONITOR_BASE_URL/api/latency/$server_id?hours=1" | \
      jq -e --arg task_id "$agent_latency_task_id" \
        '.points | any(.task_id == $task_id and .latency_ms == -1 and .packet_loss == 100)' >/dev/null; then
      failed_latency_seen=true
      break
    fi
    kill -0 "$agent_pid" 2>/dev/null || {
      cat "$agent_state_dir/agent.log" >&2
      exit 1
    }
    sleep 1
    attempt=$((attempt + 1))
  done
  [ "$failed_latency_seen" = true ] || {
    cat "$agent_state_dir/agent.log" >&2
    echo "Failed latency result was not persisted" >&2
    exit 1
  }
  kill "$agent_pid"
  wait "$agent_pid" 2>/dev/null || true
  agent_pid=
  rm -rf "$agent_state_dir"
  agent_state_dir=
  request -H "Authorization: Bearer $admin_token" -X DELETE \
    "$MONITOR_BASE_URL/api/admin/latency-tasks/$agent_latency_task_id" >/dev/null
  agent_latency_task_id=
fi

step "alerts and visibility"
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

if [ -n "${MONITOR_MIGRATION_URL:-}" ]; then
  step "database migration"
  migration_payload=$(jq -nc --arg database_url "$MONITOR_MIGRATION_URL" '{database_url:$database_url}')
  request -H "Authorization: Bearer $admin_token" \
    -H "X-NodeFlare-Password: $password_derived" -H 'Content-Type: application/json' \
    --data "$migration_payload" "$MONITOR_BASE_URL/api/admin/database/migrate" | \
    jq -e --arg kind "${MONITOR_MIGRATION_KIND:-postgresql}" \
      '.migrated_rows > 0 and .target_kind == $kind and .size_bytes > 0 and .restart_required == true' >/dev/null
fi

echo "Smoke test passed"
