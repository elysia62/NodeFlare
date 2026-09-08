import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { once } from "node:events";
import { mkdtempSync, rmSync } from "node:fs";
import { createServer as createHttpServer } from "node:http";
import { createServer as createTcpServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { WebSocketServer } from "ws";

const binary = resolve(process.env.MONITOR_AGENT_BINARY ?? "agent/target/debug/nodeflare-agent");
const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-runtime-"));
const agents = [];
const sockets = new Set();
const releaseRequests = [];
const proxy = createHttpServer((_request, response) => {
  response.writeHead(502);
  response.end();
});
proxy.on("connect", (request, socket) => {
  if (request.url === "api.github.com:443") releaseRequests.push(Date.now());
  socket.end("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
});
let websocketServer;
let upgradeServer;
let stalledServer;

async function waitUntil(predicate, timeout, message) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    assert(Date.now() < deadline, message);
    await delay(50);
  }
}

function startAgent(port, token, name) {
  const proxyUrl = `http://127.0.0.1:${proxy.address().port}`;
  const child = spawn(binary, ["-e", `http://127.0.0.1:${port}`, "-t", token], {
    env: {
      ...process.env,
      NODEFLARE_STATE_DIR: join(directory, name),
      HTTP_PROXY: proxyUrl, http_proxy: proxyUrl,
      HTTPS_PROXY: proxyUrl, https_proxy: proxyUrl,
      ALL_PROXY: proxyUrl, all_proxy: proxyUrl,
      NO_PROXY: "", no_proxy: "",
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  const running = { child, exited: once(child, "exit"), stderr: "" };
  child.stderr.on("data", (chunk) => { running.stderr += chunk; });
  agents.push(running);
  return running;
}

async function stopAgent(running) {
  if (running.child.exitCode === null && running.child.signalCode === null) {
    running.child.kill("SIGTERM");
    const timeout = setTimeout(() => running.child.kill("SIGKILL"), 3_000);
    await running.exited;
    clearTimeout(timeout);
  }
}

try {
  proxy.listen(0, "127.0.0.1");
  await once(proxy, "listening");

  console.log("agent runtime: automatic update during one-second sampling");
  websocketServer = new WebSocketServer({ noServer: true });
  upgradeServer = createHttpServer();
  let redirected = false;
  upgradeServer.on("upgrade", (request, socket, head) => {
    if (request.url === "/api/agent/ws") {
      redirected = true;
      socket.end("HTTP/1.1 307 Temporary Redirect\r\nLocation: /live\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
      return;
    }
    websocketServer.handleUpgrade(request, socket, head, (connection) => {
      websocketServer.emit("connection", connection);
    });
  });
  upgradeServer.listen(0, "127.0.0.1");
  await once(upgradeServer, "listening");
  let samples = 0;
  websocketServer.on("connection", (socket) => {
    let persistedThrough = 0;
    socket.send(JSON.stringify({
      type: "config", ts: Math.floor(Date.now() / 1_000),
      config: {
        report_interval: 15, collect_interval: 1, network_interface: "",
        agent_mirror: "", auto_update: true, latency_tasks: [],
      },
    }));
    socket.on("message", (raw) => {
      const message = JSON.parse(raw.toString());
      if (message.type !== "update") return;
      samples += message.samples.length;
      if (message.persist) persistedThrough = Math.max(...message.samples.map((sample) => sample.timestamp));
      socket.send(JSON.stringify({
        type: "ack", ts: Math.floor(Date.now() / 1_000),
        persisted: message.persist, persistenceError: false,
        persistedThroughTs: persistedThrough,
        nextPersistAfterMs: 1_000, nextWssReportAfterMs: 1_000, realtimeHint: false,
      }));
    });
  });
  let token;
  for (let index = 0; index < 100_000; index++) {
    const candidate = `runtime-update-${index}`;
    if (createHash("sha256").update(candidate).digest().readBigUInt64BE(0) % 1_801n === 2n) {
      token = candidate;
      break;
    }
  }
  assert(token, "Unable to create deterministic update jitter");
  const updating = startAgent(upgradeServer.address().port, token, "update");
  await waitUntil(() => releaseRequests.length > 0, 12_000, "Automatic update was starved by sampling");
  assert(samples > 0, "Update check must follow acknowledged samples");
  assert(redirected, "Same-origin WebSocket redirects must remain supported");
  await stopAgent(updating);

  console.log("agent runtime: remote command survives disconnect and returns its result");
  const taskId = randomUUID();
  let remoteConnections = 0;
  let received = false;
  let remoteResult;
  websocketServer.on("connection", (socket) => {
    remoteConnections++;
    socket.on("message", (raw) => {
      const message = JSON.parse(raw.toString());
      if (message.type === "task_received" && message.task_id === taskId) {
        received = true;
        socket.terminate();
      } else if (message.type === "task_result" && message.task_id === taskId) {
        remoteResult = message;
        socket.send(JSON.stringify({ type: "task_result_ack", task_id: taskId }));
      }
    });
    if (remoteConnections === 1) {
      socket.send(JSON.stringify({
        type: "remote_task", task_id: taskId,
        command: process.platform === "win32"
          ? "powershell -NoProfile -Command \"Start-Sleep -Seconds 2; Write-Output remote-finished\""
          : "sleep 2; printf remote-finished",
      }));
    }
  });
  const remote = startAgent(upgradeServer.address().port, "runtime-remote", "remote");
  await waitUntil(() => remoteResult, 15_000, "Remote command result was lost after disconnect");
  assert(received, "The Agent must acknowledge receipt before disconnecting");
  assert(remoteConnections >= 2, "The Agent must reconnect without receiving the command again");
  assert.equal(remoteResult.status, "success");
  assert.equal(remoteResult.exit_code, 0);
  assert.equal(remoteResult.result.trim(), "remote-finished");
  await stopAgent(remote);

  console.log("agent runtime: stalled handshake times out and reconnects");
  let connections = 0;
  let handshakeReceived = false;
  stalledServer = createTcpServer((socket) => {
    connections++;
    sockets.add(socket);
    socket.on("data", (data) => { handshakeReceived ||= data.toString().includes("/api/agent/ws"); });
    socket.on("error", () => {});
    socket.on("close", () => sockets.delete(socket));
  });
  stalledServer.listen(0, "127.0.0.1");
  await once(stalledServer, "listening");
  const stalled = startAgent(stalledServer.address().port, "runtime-stalled-handshake", "stalled");
  await waitUntil(() => connections >= 2, 20_000, "Stalled WebSocket handshake did not reconnect");
  assert(handshakeReceived, "The test must reach the WebSocket handshake");
  assert(stalled.stderr.includes("live connection failed"), "Handshake failure must be visible");
  console.log("Agent runtime tests passed");
} finally {
  for (const running of agents) await stopAgent(running);
  for (const socket of sockets) socket.destroy();
  if (websocketServer) {
    for (const socket of websocketServer.clients) socket.terminate();
    await new Promise((resolveClose) => websocketServer.close(resolveClose));
  }
  if (stalledServer?.listening) await new Promise((resolveClose) => stalledServer.close(resolveClose));
  if (upgradeServer?.listening) await new Promise((resolveClose) => upgradeServer.close(resolveClose));
  if (proxy.listening) await new Promise((resolveClose) => proxy.close(resolveClose));
  rmSync(directory, { recursive: true, force: true });
}
