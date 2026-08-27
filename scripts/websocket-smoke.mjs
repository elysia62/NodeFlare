import WebSocket from "ws";

const baseUrl = process.env.MONITOR_BASE_URL;
const adminToken = process.env.MONITOR_ADMIN_TOKEN;
const agentToken = process.env.MONITOR_AGENT_TOKEN;
const serverId = process.env.MONITOR_SERVER_ID;
const latencyTaskId = process.env.MONITOR_LATENCY_TASK_ID;
const expectTaskAssigned = process.env.MONITOR_EXPECT_TASK_ASSIGNED !== "0";
const configOnly = process.env.MONITOR_CONFIG_ONLY === "1";

if (!baseUrl || !adminToken || !agentToken || !serverId || !latencyTaskId) {
  throw new Error(
    "MONITOR_BASE_URL, MONITOR_ADMIN_TOKEN, MONITOR_AGENT_TOKEN, MONITOR_SERVER_ID and MONITOR_LATENCY_TASK_ID are required",
  );
}

function websocketUrl(path) {
  const url = new URL(path, baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.toString();
}

function openSocket(path, token, headers = {}) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(websocketUrl(path), {
      headers: { Authorization: `Bearer ${token}`, ...headers },
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

function expectSocketStatus(path, token, statusCode) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(websocketUrl(path), {
      headers: { Authorization: `Bearer ${token}` },
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
      response.resume();
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
    const onClose = () => fail(new Error(`WebSocket closed before receiving ${expected}`));
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

await expectSocketStatus("/api/agent/ws", "invalid-agent-token", 401);

const dashboard = configOnly ? null : await openSocket("/api/ws", adminToken);
let agent;
try {
  if (dashboard) {
    const pong = waitForMessage(dashboard, "pong");
    dashboard.send("ping");
    await pong;
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
  if (!configOnly) {
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
  const [ack] = await Promise.all([ackPromise, updatePromise]);
  if (
    ack.ts <= 0 ||
    ack.persisted !== true ||
    ack.persistenceError !== false ||
    !Number.isInteger(ack.persistedThroughTs) ||
    ack.persistedThroughTs < latestTimestamp ||
    ack.nextD1WriteAfterMs !== 120_000 ||
    ack.nextWssReportAfterMs !== 5_000
  ) {
    throw new Error(`Invalid Agent metric ACK: ${JSON.stringify(ack)}`);
  }
  }
} finally {
  await Promise.all([
    agent ? closeSocket(agent) : Promise.resolve(),
    dashboard ? closeSocket(dashboard) : Promise.resolve(),
  ]);
}
