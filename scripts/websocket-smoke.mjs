import WebSocket from "ws";

const baseUrl = process.env.MONITOR_BASE_URL;
const adminToken = process.env.MONITOR_ADMIN_TOKEN;
const agentToken = process.env.MONITOR_AGENT_TOKEN;
const serverId = process.env.MONITOR_SERVER_ID;
const latencyTaskId = process.env.MONITOR_LATENCY_TASK_ID;
const expectTaskAssigned = process.env.MONITOR_EXPECT_TASK_ASSIGNED !== "0";
const configOnly = process.env.MONITOR_CONFIG_ONLY === "1";
const expectAgentRejected = process.env.MONITOR_EXPECT_AGENT_REJECTED === "1";
const rotateAgentToken = process.env.MONITOR_ROTATE_AGENT_TOKEN === "1";
const agentProtocolHeaders = {
  "X-NodeFlare-Agent-Protocol": "1",
  "X-NodeFlare-Agent-Capabilities":
    "metrics-v1,config-v1,remote-exec-v1,task-ack-v1",
};

if (!baseUrl || !agentToken || (!expectAgentRejected && (!adminToken || !serverId || !latencyTaskId))) {
  throw new Error(
    expectAgentRejected
      ? "MONITOR_BASE_URL and MONITOR_AGENT_TOKEN are required"
      : "MONITOR_BASE_URL, MONITOR_ADMIN_TOKEN, MONITOR_AGENT_TOKEN, MONITOR_SERVER_ID and MONITOR_LATENCY_TASK_ID are required",
  );
}

function websocketUrl(path) {
  const url = new URL(path, baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

function decodeBase32(value) {
  const alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
  let buffer = 0;
  let bits = 0;
  const bytes = [];
  for (const character of value.toUpperCase()) {
    const index = alphabet.indexOf(character);
    if (index < 0) throw new Error("Invalid TOTP secret");
    buffer = (buffer << 5) | index;
    bits += 5;
    if (bits >= 8) {
      bits -= 8;
      bytes.push((buffer >> bits) & 0xff);
    }
  }
  return new Uint8Array(bytes);
}

async function currentTotp(secret) {
  const key = await crypto.subtle.importKey(
    "raw",
    decodeBase32(secret),
    { name: "HMAC", hash: "SHA-1" },
    false,
    ["sign"],
  );
  const counter = new Uint8Array(8);
  let value = BigInt(Math.floor(Date.now() / 30_000));
  for (let index = counter.length - 1; index >= 0; index -= 1) {
    counter[index] = Number(value & 0xffn);
    value >>= 8n;
  }
  const digest = new Uint8Array(await crypto.subtle.sign("HMAC", key, counter));
  const offset = digest[digest.length - 1] & 0x0f;
  const binary =
    ((digest[offset] & 0x7f) << 24) |
    (digest[offset + 1] << 16) |
    (digest[offset + 2] << 8) |
    digest[offset + 3];
  return String(binary % 1_000_000).padStart(6, "0");
}

async function setTotpEnabled(secret, enabled) {
  const response = await fetch(
    new URL(`/api/admin/2fa/${enabled ? "enable" : "disable"}`, baseUrl),
    {
      method: "POST",
      headers: {
        Authorization: `Bearer ${adminToken}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ totp_code: await currentTotp(secret) }),
    },
  );
  if (!response.ok) {
    throw new Error(
      `${enabled ? "Enable" : "Disable"} TOTP returned HTTP ${response.status}: ${await response.text()}`,
    );
  }
}

async function waitForRemoteTask(taskId, expectedStatus) {
  for (let attempt = 0; attempt < 30; attempt += 1) {
    const response = await fetch(
      new URL(`/api/admin/remote/task/${encodeURIComponent(taskId)}`, baseUrl),
      { headers: { Authorization: `Bearer ${adminToken}` } },
    );
    if (!response.ok) {
      throw new Error(`Remote task returned HTTP ${response.status}: ${await response.text()}`);
    }
    const task = await response.json();
    if (task.status === expectedStatus) return task;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`Remote task ${taskId} did not reach ${expectedStatus}`);
}

function socketHeaders(path, token, headers = {}, includeAgentProtocol = true) {
  return {
    Authorization: `Bearer ${token}`,
    ...(includeAgentProtocol && path === "/api/agent/ws" ? agentProtocolHeaders : {}),
    ...headers,
  };
}

function openSocket(path, token, headers = {}, includeAgentProtocol = true) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(websocketUrl(path), {
      headers: socketHeaders(path, token, headers, includeAgentProtocol),
    });
    const timer = setTimeout(() => {
      socket.terminate();
      reject(new Error(`${path} WebSocket handshake timed out`));
    }, 5_000);
    socket.once("open", () => {
      clearTimeout(timer);
      resolve(socket);
    });
    socket.once("unexpected-response", (_request, response) => {
      clearTimeout(timer);
      reject(new Error(`${path} WebSocket returned HTTP ${response.statusCode}`));
    });
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function expectSocketStatus(path, token, statusCode, headers = {}, includeAgentProtocol = true) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(websocketUrl(path), {
      headers: socketHeaders(path, token, headers, includeAgentProtocol),
    });
    const timer = setTimeout(() => {
      socket.terminate();
      reject(new Error(`${path} did not reject the WebSocket handshake`));
    }, 5_000);
    socket.once("open", () => {
      clearTimeout(timer);
      socket.terminate();
      reject(new Error(`${path} unexpectedly accepted the WebSocket handshake`));
    });
    socket.once("unexpected-response", (_request, response) => {
      clearTimeout(timer);
      response.destroy();
      socket.terminate();
      if (response.statusCode === statusCode) {
        resolve();
      } else {
        reject(new Error(`${path} returned HTTP ${response.statusCode}, expected ${statusCode}`));
      }
    });
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function waitForMessage(socket, expected) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`WebSocket did not receive ${expected}`)), 5_000);
    socket.once("message", (data) => {
      clearTimeout(timer);
      const message = data.toString();
      if (message !== expected) reject(new Error(`Expected ${expected}, received ${message}`));
      else resolve();
    });
    socket.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
  });
}

function waitForJsonMessage(socket, expected, predicate) {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timer);
      socket.off("message", onMessage);
      socket.off("close", onClose);
      socket.off("error", onError);
    };
    const fail = (error) => {
      cleanup();
      reject(error);
    };
    const onMessage = (data) => {
      let message;
      try {
        message = JSON.parse(data.toString());
      } catch {
        return;
      }
      if (!predicate(message)) return;
      cleanup();
      resolve(message);
    };
    const onClose = (code, reason) => {
      const detail = reason.length ? `: ${reason.toString()}` : "";
      fail(new Error(`WebSocket closed with code ${code}${detail} before receiving ${expected}`));
    };
    const onError = (error) => fail(error);
    const timer = setTimeout(
      () => fail(new Error(`WebSocket did not receive ${expected}`)),
      5_000,
    );
    socket.on("message", onMessage);
    socket.once("close", onClose);
    socket.once("error", onError);
  });
}

function closeSocket(socket) {
  return new Promise((resolve, reject) => {
    if (socket.readyState === WebSocket.CLOSED) {
      resolve();
      return;
    }
    const cleanup = () => {
      clearTimeout(timer);
      socket.off("close", onClose);
      socket.off("error", onError);
    };
    const onClose = () => {
      cleanup();
      resolve();
    };
    const onError = (error) => {
      cleanup();
      reject(error);
    };
    const timer = setTimeout(() => {
      cleanup();
      socket.terminate();
      reject(new Error("WebSocket close timed out"));
    }, 5_000);
    socket.once("close", onClose);
    socket.once("error", onError);
    if (socket.readyState === WebSocket.OPEN) socket.close();
  });
}

function waitForSocketClose(socket, expected) {
  return new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(timer);
      socket.off("close", onClose);
      socket.off("error", onError);
    };
    const onClose = () => {
      cleanup();
      resolve();
    };
    const onError = (error) => {
      cleanup();
      reject(error);
    };
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error(`WebSocket did not close after ${expected}`));
    }, 5_000);
    socket.once("close", onClose);
    socket.once("error", onError);
  });
}

await expectSocketStatus("/api/agent/ws", "invalid-agent-token", 426, {}, false);
await expectSocketStatus(
  "/api/agent/ws",
  "invalid-agent-token",
  426,
  { "X-NodeFlare-Agent-Protocol": "999" },
);
await expectSocketStatus(
  "/api/agent/ws",
  "invalid-agent-token",
  426,
  {
    "X-NodeFlare-Agent-Capabilities":
      "metrics-v1,config-v1,remote-exec-v1",
  },
);
await expectSocketStatus("/api/agent/ws", "invalid-agent-token", 401);
if (expectAgentRejected) {
  await expectSocketStatus("/api/agent/ws", agentToken, 401);
  process.exit(0);
}

const dashboard = configOnly ? null : await openSocket("/api/ws", adminToken);
let agent;
let enabledTotpSecret = "";
try {
  if (dashboard) {
    const pong = waitForMessage(dashboard, "pong");
    dashboard.send("ping");
    await pong;

    const oversizedDashboard = await openSocket("/api/ws", adminToken);
    const oversizedClosed = waitForSocketClose(
      oversizedDashboard,
      "an oversized dashboard message was sent",
    );
    oversizedDashboard.send("x".repeat(16 * 1024));
    await oversizedClosed;
  }

  agent = await openSocket("/api/agent/ws", agentToken, { "CF-Connecting-IP": "8.8.8.8" });
  const config = await waitForJsonMessage(
    agent,
    "Agent config",
    (message) => message.type === "config" && message.config,
  );
  const hasTask = config.config.latency_tasks?.some(
    (task) =>
      task.id === latencyTaskId &&
      task.task_type === "tcp" &&
      task.target === "example.com" &&
      task.port === 443,
  );
  if (
    config.config.collect_interval !== 5 ||
    config.config.agent_mirror !== "https://mirror.example.com" ||
    config.config.auto_update !== true ||
    hasTask !== expectTaskAssigned
  ) {
    throw new Error(`Invalid Agent config: ${JSON.stringify(config)}`);
  }
  if (rotateAgentToken) {
    const firstSocket = agent;
    const firstClosed = waitForSocketClose(firstSocket, "a duplicate Agent connected");
    const replacement = await openSocket("/api/agent/ws", agentToken);
    await waitForJsonMessage(
      replacement,
      "replacement Agent config",
      (message) => message.type === "config" && message.config,
    );
    await firstClosed;
    agent = replacement;

    const rotatedClosed = waitForSocketClose(agent, "the Agent Token rotated");
    const rotateResponse = await fetch(
      new URL(`/api/admin/servers/${encodeURIComponent(serverId)}/token`, baseUrl),
      {
        method: "POST",
        headers: { Authorization: `Bearer ${adminToken}` },
      },
    );
    if (!rotateResponse.ok) {
      throw new Error(
        `Agent Token rotation returned HTTP ${rotateResponse.status}: ${await rotateResponse.text()}`,
      );
    }
    const rotated = await rotateResponse.json();
    const rotatedToken = rotated.agent_token;
    if (!rotatedToken || rotatedToken === agentToken) {
      throw new Error("Agent Token rotation did not return a new token");
    }
    await rotatedClosed;
    await expectSocketStatus("/api/agent/ws", agentToken, 401);

    const rotatedSocket = await openSocket("/api/agent/ws", rotatedToken);
    await waitForJsonMessage(
      rotatedSocket,
      "rotated Agent config",
      (message) => message.type === "config" && message.config,
    );
    await closeSocket(rotatedSocket);
    process.stdout.write(`${rotatedToken}\n`);
  } else if (!configOnly) {
  const setupResponse = await fetch(new URL("/api/admin/2fa/setup", baseUrl), {
    method: "POST",
    headers: { Authorization: `Bearer ${adminToken}` },
  });
  if (!setupResponse.ok) {
    throw new Error(`TOTP setup returned HTTP ${setupResponse.status}: ${await setupResponse.text()}`);
  }
  const setup = await setupResponse.json();
  if (!setup.secret) throw new Error("TOTP setup did not return a secret");
  await setTotpEnabled(setup.secret, true);
  enabledTotpSecret = setup.secret;

  const command = "printf nodeflare-smoke";
  const assignedTaskPromise = waitForJsonMessage(
    agent,
    "remote task",
    (message) => message.type === "remote_task" && message.command === command,
  );
  const createTaskResponse = await fetch(new URL("/api/admin/remote/task", baseUrl), {
    method: "POST",
    headers: {
      Authorization: `Bearer ${adminToken}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      command,
      server_ids: [serverId],
      totp_code: await currentTotp(setup.secret),
    }),
  });
  if (!createTaskResponse.ok) {
    throw new Error(
      `Remote task creation returned HTTP ${createTaskResponse.status}: ${await createTaskResponse.text()}`,
    );
  }
  const createdTask = await createTaskResponse.json();
  const taskId = createdTask.tasks?.[0]?.task_id;
  const assignedTask = await assignedTaskPromise;
  if (!taskId || assignedTask.task_id !== taskId) {
    throw new Error(`Invalid remote task assignment: ${JSON.stringify(assignedTask)}`);
  }

  agent.send(JSON.stringify({ type: "task_received", task_id: taskId }));
  await waitForRemoteTask(taskId, "sent");
  const result = {
    type: "task_result",
    task_id: taskId,
    status: "success",
    result: "nodeflare-smoke",
    exit_code: 0,
  };
  const resultAckPromise = waitForJsonMessage(
    agent,
    "remote task result ACK",
    (message) => message.type === "task_result_ack" && message.task_id === taskId,
  );
  agent.send(JSON.stringify(result));
  await resultAckPromise;
  const completedTask = await waitForRemoteTask(taskId, "success");
  if (completedTask.result !== "nodeflare-smoke" || completedTask.exit_code !== 0) {
    throw new Error(`Invalid remote task result: ${JSON.stringify(completedTask)}`);
  }

  const duplicateAckPromise = waitForJsonMessage(
    agent,
    "duplicate remote task result ACK",
    (message) => message.type === "task_result_ack" && message.task_id === taskId,
  );
  agent.send(JSON.stringify(result));
  await duplicateAckPromise;

  await setTotpEnabled(setup.secret, false);
  enabledTotpSecret = "";

  const wakeHintPromise = waitForJsonMessage(
    agent,
    "batched overview wake hint",
    (message) => message.type === "ack" && message.realtimeHint === true,
  );
  const wakeResponse = await fetch(new URL("/api/live/wake", baseUrl), {
    method: "POST",
    headers: {
      Authorization: `Bearer ${adminToken}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ server_ids: [serverId] }),
  });
  if (wakeResponse.status !== 204) {
    throw new Error(`Batch wake returned HTTP ${wakeResponse.status}: ${await wakeResponse.text()}`);
  }
  const wakeHint = await wakeHintPromise;
  if (wakeHint.nextWssReportAfterMs !== 5_000) {
    throw new Error(`Invalid batch wake hint: ${JSON.stringify(wakeHint)}`);
  }

  const timestamp = Math.floor(Date.now() / 1_000);
  const baseSample = {
    timestamp,
    cpu: 18.5,
    load1: 0.42,
    load5: 0.36,
    load15: 0.31,
    mem_used: 2147483648,
    mem_total: 4294967296,
    swap_used: 0,
    swap_total: 0,
    disk_used: 21474836480,
    disk_total: 53687091200,
    net_in: 4096,
    net_out: 2048,
    net_rx_total: 1073741824,
    net_tx_total: 536870912,
    uptime: 86400,
    processes: 90,
    tcp_connections: 18,
    udp_connections: 4,
    cpu_cores: 2,
    cpu_model: "Smoke CPU",
    os: "Debian 12",
    kernel: "6.1",
    arch: "x86_64",
    virtualization: "kvm",
    gpu_usage: 32.5,
    gpu_model: "NVIDIA T4",
    agent_version: "smoke",
    ip_v4: "203.0.113.7",
    ip_v6: "2001:db8::1",
    disk_read_bps: 4194304,
    disk_write_bps: 2097152,
    disk_read_iops: 120,
    disk_write_iops: 48,
    disk_await_ms: 1.4,
    disk_utilization: 8.2,
    disks: [
      {
        name: "/dev/vda1",
        mount_point: "/",
        used: 21474836480,
        total: 53687091200,
        read_bps: 4194304,
        write_bps: 2097152,
        read_iops: 120,
        write_iops: 48,
        await_ms: 1.4,
        utilization: 8.2,
      },
    ],
    gpus: [
      { model: "NVIDIA T4", usage: 32.5, memory_used: 1073741824, memory_total: 17179869184 },
    ],
    latency_results: [],
  };
  const sample = (offset, overrides = {}) => ({
    ...structuredClone(baseSample),
    timestamp: timestamp + offset,
    ...overrides,
  });
  const samples = [
    sample(0),
    sample(1, {
      latency_results: [
        { task_id: latencyTaskId, timestamp: timestamp + 1, latency_ms: 28.4, packet_loss: 25 },
      ],
    }),
    sample(2, {
      latency_results: [
        { task_id: latencyTaskId, timestamp: timestamp + 2, latency_ms: 48.4, packet_loss: 75 },
      ],
    }),
    sample(3, { net_rx_total: 2147483648, net_tx_total: 1073741824 }),
    sample(4, { net_rx_total: 268435456, net_tx_total: 134217728 }),
    sample(5, { net_rx_total: 536870912, net_tx_total: 268435456 }),
  ];
  const latestTimestamp = timestamp + 5;
  const ackPromise = waitForJsonMessage(
    agent,
    "Agent metric ACK",
    (message) => message.type === "ack" && message.realtimeHint === false,
  );
  const updatePromise = waitForJsonMessage(
    dashboard,
    "dashboard batchUpdate",
    (message) =>
      message.type === "batchUpdate" &&
      message.updates?.some(
        (update) =>
          update.serverId === serverId &&
          update.samples?.some(
            (entry) => entry.ts === latestTimestamp && entry.data?.cpu === 18.5,
          ),
      ),
  );
  agent.send(JSON.stringify({ type: "update", batchId: `smoke-${timestamp}`, samples }));
  const [ack, update] = await Promise.all([ackPromise, updatePromise]);
  if (
    ack.ts <= 0 ||
    ack.persisted !== true ||
    ack.persistenceError !== false ||
    !Number.isInteger(ack.persistedThroughTs) ||
    ack.persistedThroughTs < latestTimestamp ||
    ack.nextPersistAfterMs !== 60_000 ||
    ack.nextWssReportAfterMs !== 5_000
  ) {
    throw new Error(`Invalid Agent metric ACK: ${JSON.stringify(ack)}`);
  }

  const publicIdentityFields = [
    "cpu_model",
    "os",
    "kernel",
    "arch",
    "virtualization",
    "gpu_model",
    "agent_version",
  ];
  const browserOmittedFields = [
    ...publicIdentityFields,
    "ip_v4",
    "ip_v6",
  ];
  for (const entry of update.updates.flatMap((u) => u.samples ?? [])) {
    const leaked = browserOmittedFields.filter((field) => entry.data?.[field] !== undefined);
    if (leaked.length) {
      throw new Error(`Live sample leaked identity fields: ${leaked.join(", ")}`);
    }
    if (entry.data?.cpu === undefined) {
      throw new Error("Live sample lost a dynamic field (cpu)");
    }
  }
  const bootstrap = await fetch(new URL("/api/bootstrap", baseUrl)).then((r) => r.json());
  const persisted = bootstrap.servers?.find((s) => s.id === serverId);
  if (!persisted) {
    throw new Error(`Bootstrap missing server ${serverId}`);
  }
  const missing = publicIdentityFields.filter((field) => !persisted[field]);
  if (missing.length) {
    throw new Error(`Bootstrap missing identity fields: ${missing.join(", ")}`);
  }
  const publicIpLeak = ["ip_v4", "ip_v6"].filter((field) => persisted[field] !== undefined);
  if (publicIpLeak.length) {
    throw new Error(`Bootstrap leaked Agent public IP fields: ${publicIpLeak.join(", ")}`);
  }

  const adminServersResponse = await fetch(new URL("/api/admin/servers", baseUrl), {
    headers: { Authorization: `Bearer ${adminToken}` },
  });
  if (!adminServersResponse.ok) {
    throw new Error(`Admin servers returned HTTP ${adminServersResponse.status}`);
  }
  const adminServers = await adminServersResponse.json();
  const adminPersisted = adminServers.servers?.find((server) => server.id === serverId);
  if (
    adminPersisted?.ip_v4 !== baseSample.ip_v4 ||
    adminPersisted?.ip_v6 !== baseSample.ip_v6
  ) {
    throw new Error(`Admin servers returned invalid public IP fields: ${JSON.stringify(adminPersisted)}`);
  }
  }
} finally {
  if (enabledTotpSecret) {
    try {
      await setTotpEnabled(enabledTotpSecret, false);
    } catch (error) {
      console.error(error);
    }
  }
  await Promise.all([
    agent ? closeSocket(agent) : Promise.resolve(),
    dashboard ? closeSocket(dashboard) : Promise.resolve(),
  ]);
}
