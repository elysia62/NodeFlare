use super::*;

pub(crate) struct LiveSender {
    pub(crate) pending: Arc<(Mutex<VecDeque<Report>>, Condvar)>,
    pub(crate) send_interval: Arc<Mutex<Duration>>,
    pub(crate) persisted_through: Arc<AtomicI64>,
    pub(crate) remote_config: Arc<Mutex<Option<RemoteConfig>>>,
    pub(crate) stats: Arc<runtime_stats::RuntimeStats>,
}

pub(crate) struct LiveSenderWorker {
    pub(crate) pending: Arc<(Mutex<VecDeque<Report>>, Condvar)>,
    pub(crate) configured_interval: Arc<Mutex<Duration>>,
    pub(crate) persisted_through: Arc<AtomicI64>,
    pub(crate) remote_config: Arc<Mutex<Option<RemoteConfig>>>,
    pub(crate) clock: SharedClock,
    pub(crate) stats: Arc<runtime_stats::RuntimeStats>,
}

impl LiveSender {
    pub(crate) fn start(
        config: &RuntimeConfig,
        clock: SharedClock,
        stats: Arc<runtime_stats::RuntimeStats>,
    ) -> Result<Self> {
        let pending = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
        let send_interval = Arc::new(Mutex::new(live_batch_interval(config.collect_interval)));
        let persisted_through = Arc::new(AtomicI64::new(0));
        let remote_config = Arc::new(Mutex::new(None));
        let endpoint = live_endpoint(&config.endpoint)?;
        let token = config.token.clone();
        let sender_state = Arc::clone(&pending);
        let sender_interval = Arc::clone(&send_interval);
        let sender_persisted_through = Arc::clone(&persisted_through);
        let sender_remote_config = Arc::clone(&remote_config);
        let sender_stats = Arc::clone(&stats);
        thread::Builder::new()
            .name("nodeflare-live".to_string())
            .spawn(move || {
                live_sender_loop(
                    &endpoint,
                    &token,
                    LiveSenderWorker {
                        pending: sender_state,
                        configured_interval: sender_interval,
                        persisted_through: sender_persisted_through,
                        remote_config: sender_remote_config,
                        clock,
                        stats: sender_stats,
                    },
                )
            })?;
        Ok(Self {
            pending,
            send_interval,
            persisted_through,
            remote_config,
            stats,
        })
    }

    pub(crate) fn send(&self, report: &Report) {
        let (pending, ready) = &*self.pending;
        if let Ok(mut pending) = pending.lock() {
            if pending.len() >= LIVE_QUEUE_CAPACITY {
                pending.pop_front();
                self.stats.live_queue_dropped(1);
            }
            pending.push_back(report.clone());
            ready.notify_one();
        }
    }

    pub(crate) fn set_send_interval(&self, collect_interval: u64) {
        if let Ok(mut interval) = self.send_interval.lock() {
            *interval = live_batch_interval(collect_interval);
        }
        self.pending.1.notify_one();
    }

    pub(crate) fn persisted_through(&self) -> i64 {
        self.persisted_through.load(Ordering::Acquire)
    }

    pub(crate) fn take_remote_config(&self) -> Option<RemoteConfig> {
        self.remote_config
            .lock()
            .ok()
            .and_then(|mut config| config.take())
    }
}

pub(crate) fn live_batch_interval(collect_interval: u64) -> Duration {
    Duration::from_secs(collect_interval.clamp(telemetry::MIN_UPLOAD_INTERVAL, 60))
}

pub(crate) fn live_endpoint(endpoint: &str) -> Result<String> {
    let mut url = url::Url::parse(endpoint)?;
    url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
        .map_err(|_| "unsupported live endpoint scheme")?;
    let path = format!("{}/api/agent/ws", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url.to_string())
}

pub(crate) type LiveSocket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

pub(crate) fn connect_live(endpoint: &str, token: &str) -> Result<(LiveSocket, Option<i64>)> {
    let started_ms = unix_timestamp_millis();
    let deadline = Instant::now() + LIVE_CONNECT_TIMEOUT;
    let mut url = url::Url::parse(endpoint)?;
    let mut redirects = 0;
    let (socket, response) = loop {
        let mut request = url.as_str().into_client_request()?;
        request
            .headers_mut()
            .insert("Authorization", format!("Bearer {token}").parse()?);
        request
            .headers_mut()
            .insert("User-Agent", format!("nodeflare-agent/{VERSION}").parse()?);
        request
            .headers_mut()
            .insert(AGENT_PROTOCOL_HEADER, AGENT_PROTOCOL_VERSION.parse()?);
        request.headers_mut().insert(
            AGENT_CAPABILITIES_HEADER,
            REQUIRED_AGENT_CAPABILITIES.join(",").parse()?,
        );
        let stream = connect_live_stream(&url, deadline)?;
        match client_tls(request, stream) {
            Ok(connected) => break connected,
            Err(tungstenite::HandshakeError::Failure(WebSocketError::Http(response)))
                if response.status().is_redirection() && redirects < 3 =>
            {
                let location = response
                    .headers()
                    .get("location")
                    .ok_or("WebSocket redirect has no location")?
                    .to_str()?;
                let redirected = url.join(location)?;
                // Never forward the Agent token to a different origin.
                if redirected.origin() != url.origin() {
                    return Err("WebSocket redirect must stay on the configured origin".into());
                }
                url = redirected;
                redirects += 1;
            }
            Err(error) => return Err(format!("WebSocket handshake failed: {error}").into()),
        }
    };
    let ended_ms = unix_timestamp_millis();
    let clock_offset_ms = response
        .headers()
        .get("date")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| clock_offset_from_http_date(value, started_ms, ended_ms));
    if !response
        .headers()
        .get(AGENT_PROTOCOL_HEADER)
        .is_some_and(|value| value == AGENT_PROTOCOL_VERSION)
    {
        return Err(format!(
            "backend protocol mismatch; update the backend to protocol {AGENT_PROTOCOL_VERSION}"
        )
        .into());
    }
    Ok((socket, clock_offset_ms))
}

pub(crate) fn connect_live_stream(url: &url::Url, deadline: Instant) -> Result<TcpStream> {
    let host = url
        .host_str()
        .ok_or("WebSocket endpoint has no hostname")?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let port = url
        .port_or_known_default()
        .ok_or("WebSocket endpoint has no port")?;
    let addresses = (host, port).to_socket_addrs()?;
    let mut connected = None;
    let mut last_error = io::Error::new(
        ErrorKind::NotFound,
        "WebSocket hostname resolved to no addresses",
    );
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(
                io::Error::new(ErrorKind::TimedOut, "WebSocket connection timed out").into(),
            );
        }
        match TcpStream::connect_timeout(&address, remaining.min(Duration::from_secs(3))) {
            Ok(stream) => {
                connected = Some(stream);
                break;
            }
            Err(error) => last_error = error,
        }
    }
    let stream = connected.ok_or(last_error)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(ErrorKind::TimedOut, "WebSocket connection timed out").into());
    }
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(remaining))?;
    stream.set_write_timeout(Some(remaining))?;
    Ok(stream)
}

pub(crate) fn set_live_read_timeout(socket: &mut LiveSocket, timeout: Option<Duration>) -> io::Result<()> {
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream.sock.set_read_timeout(timeout)
        }
        // An unmatched TLS backend keeps running without a read timeout rather
        // than failing outright; the rustls-only feature set always matches an
        // arm above, so this is defensive.
        _ => Ok(()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveAck {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) ts: i64,
    pub(crate) persisted: bool,
    #[serde(rename = "persistenceError")]
    pub(crate) persistence_error: bool,
    #[serde(rename = "persistedThroughTs")]
    pub(crate) persisted_through_ts: i64,
    #[serde(rename = "nextPersistAfterMs")]
    pub(crate) next_persist_after_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LiveConfigMessage {
    #[serde(rename = "type")]
    pub(crate) message_type: String,
    pub(crate) ts: i64,
    pub(crate) config: RemoteConfig,
}

pub(crate) fn ack_persist_interval(ack: &LiveAck) -> Duration {
    Duration::from_millis(ack.next_persist_after_ms)
        .clamp(Duration::from_secs(1), Duration::from_secs(3600))
}

pub(crate) enum LiveRead {
    Ack(LiveAck),
    Config(RemoteConfig),
    RemoteTask(RemoteTaskMessage),
    TaskResultAck(String),
    Closed,
    Pending,
}

pub(crate) fn read_live_ack(socket: &mut LiveSocket) -> Result<LiveRead> {
    match socket.read() {
        Ok(Message::Text(text)) => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text.as_ref()) else {
                return Ok(LiveRead::Pending);
            };
            match value.get("type").and_then(serde_json::Value::as_str) {
                Some("ack") => {
                    let Ok(ack) = serde_json::from_value::<LiveAck>(value) else {
                        return Ok(LiveRead::Pending);
                    };
                    if ack.message_type != "ack" || ack.ts <= 0 {
                        return Ok(LiveRead::Pending);
                    }
                    Ok(LiveRead::Ack(ack))
                }
                Some("config") => {
                    let Ok(message) = serde_json::from_value::<LiveConfigMessage>(value) else {
                        return Ok(LiveRead::Pending);
                    };
                    if message.message_type != "config" || message.ts <= 0 {
                        return Ok(LiveRead::Pending);
                    }
                    Ok(LiveRead::Config(message.config))
                }
                Some("remote_task") => {
                    let Ok(task) = serde_json::from_value::<RemoteTaskMessage>(value) else {
                        return Ok(LiveRead::Pending);
                    };
                    if task.message_type != "remote_task"
                        || !valid_remote_task_id(&task.task_id)
                        || task.command.trim().is_empty()
                        || task.command.len() > 16_384
                    {
                        return Ok(LiveRead::Pending);
                    }
                    Ok(LiveRead::RemoteTask(task))
                }
                Some("task_result_ack") => {
                    let Ok(ack) = serde_json::from_value::<TaskResultAckMessage>(value) else {
                        return Ok(LiveRead::Pending);
                    };
                    if ack.message_type != "task_result_ack" || !valid_remote_task_id(&ack.task_id)
                    {
                        return Ok(LiveRead::Pending);
                    }
                    Ok(LiveRead::TaskResultAck(ack.task_id))
                }
                _ => Ok(LiveRead::Pending),
            }
        }
        Ok(Message::Ping(payload)) => {
            socket.send(Message::Pong(payload))?;
            Ok(LiveRead::Pending)
        }
        Ok(Message::Close(_)) => Ok(LiveRead::Closed),
        Ok(_) => Ok(LiveRead::Pending),
        Err(WebSocketError::Io(error))
            if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
        {
            Ok(LiveRead::Pending)
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn accept_remote_task(
    pub(crate) socket: &mut LiveSocket,
    pub(crate) executor: &mut RemoteExecutor,
    pub(crate) task: RemoteTaskMessage,
) -> Result<()> {
    let task_id = task.task_id.clone();
    let rejected = executor.enqueue(task);
    socket.send(Message::Text(
        serde_json::json!({
            "type": "task_received",
            "task_id": task_id,
        })
        .to_string()
        .into(),
    ))?;
    if let Some(result) = rejected {
        socket.send(Message::Text(serde_json::to_string(&result)?.into()))?;
    }
    flush_remote_results(socket, executor)?;
    Ok(())
}

pub(crate) fn wait_for_live_ack(
    pub(crate) socket: &mut LiveSocket,
    pub(crate) remote_config: &Arc<Mutex<Option<RemoteConfig>>>,
    pub(crate) remote_executor: &mut RemoteExecutor,
) -> Result<LiveRead> {
    let deadline = Instant::now() + LIVE_ACK_READ_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(LiveRead::Pending);
        }
        set_live_read_timeout(socket, Some(remaining))?;
        match read_live_ack(socket)? {
            LiveRead::Ack(ack) => {
                return Ok(LiveRead::Ack(ack));
            }
            LiveRead::Config(config) => {
                if let Ok(mut target) = remote_config.lock() {
                    *target = Some(config);
                }
            }
            LiveRead::RemoteTask(task) => {
                accept_remote_task(socket, remote_executor, task)?;
            }
            LiveRead::TaskResultAck(task_id) => {
                remote_executor.acknowledge(&task_id);
            }
            LiveRead::Closed => return Ok(LiveRead::Closed),
            LiveRead::Pending => {}
        }
    }
}

pub(crate) fn live_update_payload(
    pub(crate) reports: Vec<Report>,
    pub(crate) persist: bool,
    pub(crate) info: &mut Option<telemetry::Info>,
) -> Result<Vec<u8>> {
    let mut next_info = info.clone();
    let payload = telemetry::encode(&telemetry::Update::from_reports(
        reports,
        persist,
        &mut next_info,
    ))?;
    *info = next_info;
    Ok(payload)
}

pub(crate) fn observe_persisted_through(target: &AtomicI64, ack: &LiveAck) {
    if ack.persisted && !ack.persistence_error {
        target.fetch_max(ack.persisted_through_ts.max(0), Ordering::AcqRel);
    }
}

pub(crate) fn prune_live_queue(pending: &Arc<(Mutex<VecDeque<Report>>, Condvar)>, persisted_through: i64) {
    if persisted_through <= 0 {
        return;
    }
    if let Ok(mut queue) = pending.0.lock() {
        queue.retain(|report| report.timestamp > persisted_through);
    }
}

pub(crate) fn live_batch_after(queue: &VecDeque<Report>, timestamp: i64) -> Vec<Report> {
    live_batch::batch_from(queue.iter().filter(|report| report.timestamp > timestamp))
}

/// Bounded exponential backoff for the live connection, with jitter derived
/// from the token so a fleet of agents that dropped together does not retry in
/// lockstep and hammer an unreachable server.
pub(crate) fn reconnect_delay(attempt: u32, token: &str) -> Duration {
    let base_ms = LIVE_RECONNECT_DELAY.as_millis() as u64;
    let backoff_ms = base_ms
        .saturating_mul(1_u64 << attempt.min(5))
        .min(LIVE_RECONNECT_MAX_DELAY.as_millis() as u64);
    let digest = Sha256::digest(token.as_bytes());
    let seed = u64::from(digest[0]) ^ (u64::from(attempt) << 8);
    let jitter_ms = seed % (backoff_ms / 4 + 1);
    Duration::from_millis(backoff_ms + jitter_ms)
}

pub(crate) struct ReconnectBackoff {
    pub(crate) attempt: u32,
    pub(crate) token: String,
}

impl ReconnectBackoff {
    pub(crate) fn new(token: &str) -> Self {
        Self {
            attempt: 0,
            token: token.to_string(),
        }
    }

    /// Reset after a confirmed round trip, not merely after a TCP connect, so a
    /// server that accepts then immediately drops connections cannot keep us at
    /// the shortest delay.
    pub(crate) fn reset(&mut self) {
        self.attempt = 0;
    }

    pub(crate) fn next_delay(&mut self) -> Duration {
        self.attempt = self.attempt.saturating_add(1);
        reconnect_delay(self.attempt, &self.token)
    }
}

pub(crate) fn live_sender_loop(endpoint: &str, token: &str, worker: LiveSenderWorker) {
    let LiveSenderWorker {
        pending,
        configured_interval,
        persisted_through,
        remote_config,
        clock,
        stats,
    } = worker;
    let mut socket: Option<LiveSocket> = None;
    let mut backoff = ReconnectBackoff::new(token);
    let mut wss_interval = configured_interval.lock().map_or(
        Duration::from_secs(telemetry::MIN_UPLOAD_INTERVAL),
        |interval| *interval,
    );
    let mut info = None;
    let mut accepted_through = persisted_through.load(Ordering::Acquire);
    let mut next_send_at = Instant::now();
    let mut next_probe_at = Instant::now();
    let mut remote_executor = match RemoteExecutor::new() {
        Ok(executor) => executor,
        Err(error) => {
            eprintln!("remote executor disabled: {error}");
            RemoteExecutor::disabled()
        }
    };
    'sender: loop {
        if socket.is_none() {
            match connect_live(endpoint, token) {
                Ok((mut connected, offset_ms)) => {
                    observe_clock(&clock, offset_ms);
                    if set_live_read_timeout(&mut connected, Some(LIVE_HINT_READ_TIMEOUT)).is_err()
                    {
                        thread::sleep(backoff.next_delay());
                        continue;
                    }
                    socket = Some(connected);
                    info = None;
                    remote_executor.reset_delivery();
                    accepted_through = persisted_through.load(Ordering::Acquire);
                    next_send_at = Instant::now();
                    next_probe_at = Instant::now();
                }
                Err(error) => {
                    stats.live_connect_failed();
                    eprintln!("live connection failed: {error}");
                    thread::sleep(backoff.next_delay());
                    continue;
                }
            }
        }

        if let Some(connected) = socket.as_mut()
            && flush_remote_results(connected, &mut remote_executor).is_err()
        {
            socket = None;
            thread::sleep(backoff.next_delay());
            continue;
        }

        let (queue_lock, ready) = &*pending;
        let Ok(mut queue) = queue_lock.lock() else {
            return;
        };
        let (batch, persist, more_pending) = loop {
            let now = Instant::now();
            let persisted = persisted_through.load(Ordering::Acquire);
            let has_unsent = queue.iter().any(|report| report.timestamp > accepted_through);
            let persist =
                now >= next_probe_at && queue.iter().any(|report| report.timestamp > persisted);
            if persist || (has_unsent && now >= next_send_at) {
                // Build (and serialize) the batch only once it is actually due;
                // the wait below re-runs several times per second while idle.
                let unsent = live_batch_after(&queue, accepted_through);
                let last = unsent
                    .last()
                    .map_or(accepted_through, |report| report.timestamp);
                let more_pending = queue.iter().any(|report| report.timestamp > last);
                // A commit can be empty: samples already sent remain buffered by the server.
                break (unsent, persist || more_pending, more_pending);
            }

            let wake_at = if has_unsent {
                next_send_at.min(next_probe_at)
            } else if queue.iter().any(|report| report.timestamp > persisted) {
                next_probe_at
            } else {
                now + Duration::from_millis(250)
            };
            let wait = wake_at
                .saturating_duration_since(now)
                .min(Duration::from_millis(250));
            queue = match ready.wait_timeout(queue, wait) {
                Ok((queue, _)) => queue,
                Err(_) => return,
            };
            drop(queue);

            let mut drop_socket = false;
            if let Some(connected) = socket.as_mut() {
                if flush_remote_results(connected, &mut remote_executor).is_err()
                    || set_live_read_timeout(connected, Some(LIVE_HINT_READ_TIMEOUT)).is_err()
                {
                    drop_socket = true;
                } else {
                    match read_live_ack(connected) {
                        Ok(LiveRead::Closed) => drop_socket = true,
                        Ok(LiveRead::Ack(ack)) => {
                            observe_persisted_through(&persisted_through, &ack);
                            if ack.persistence_error {
                                stats.live_persistence_failed();
                                drop_socket = true;
                            } else {
                                prune_live_queue(&pending, ack.persisted_through_ts);
                                accepted_through = accepted_through.max(ack.persisted_through_ts);
                                next_probe_at = Instant::now() + ack_persist_interval(&ack);
                            }
                        }
                        Ok(LiveRead::Config(config)) => {
                            if let Ok(mut target) = remote_config.lock() {
                                *target = Some(config);
                            }
                        }
                        Ok(LiveRead::RemoteTask(task)) => {
                            if accept_remote_task(connected, &mut remote_executor, task).is_err() {
                                drop_socket = true;
                            }
                        }
                        Ok(LiveRead::TaskResultAck(task_id)) => {
                            remote_executor.acknowledge(&task_id);
                        }
                        Ok(LiveRead::Pending) => {}
                        Err(error) => {
                            eprintln!("live hint read failed: {error}");
                            drop_socket = true;
                        }
                    }
                }
            }
            if drop_socket {
                socket = None;
                accepted_through = persisted_through.load(Ordering::Acquire);
                next_send_at = Instant::now();
                next_probe_at = Instant::now();
                thread::sleep(backoff.next_delay());
                continue 'sender;
            }
            queue = match queue_lock.lock() {
                Ok(queue) => queue,
                Err(_) => return,
            };
        };
        drop(queue);

        if let Ok(interval) = configured_interval.lock() {
            wss_interval = (*interval).clamp(
                Duration::from_secs(telemetry::MIN_UPLOAD_INTERVAL),
                Duration::from_secs(60),
            );
        }

        let batch_last = batch.last().map(|report| report.timestamp);
        let batch_len = batch.len();
        let payload = match live_update_payload(batch, persist, &mut info) {
            Ok(payload) => payload,
            Err(error) => {
                eprintln!("live payload encode failed: {error}");
                next_send_at = Instant::now() + wss_interval;
                continue;
            }
        };
        let sent = socket
            .as_mut()
            .is_some_and(|socket| socket.send(Message::Binary(payload.into())).is_ok());
        if !sent {
            socket = None;
            accepted_through = persisted_through.load(Ordering::Acquire);
            next_send_at = Instant::now();
            next_probe_at = Instant::now();
            thread::sleep(backoff.next_delay());
            continue;
        }
        stats.live_batch_sent(batch_len);
        backoff.reset();
        if let Some(timestamp) = batch_last {
            accepted_through = accepted_through.max(timestamp);
        }
        next_send_at = Instant::now() + wss_interval;
        if !persist {
            continue;
        }

        let mut drop_socket = false;
        if let Some(socket) = socket.as_mut() {
            match wait_for_live_ack(socket, &remote_config, &mut remote_executor) {
                Ok(LiveRead::Closed) => drop_socket = true,
                Ok(LiveRead::Ack(ack)) => {
                    observe_persisted_through(&persisted_through, &ack);
                    if ack.persistence_error {
                        stats.live_persistence_failed();
                        drop_socket = true;
                    } else {
                        prune_live_queue(&pending, ack.persisted_through_ts);
                        accepted_through = accepted_through.max(ack.persisted_through_ts);
                        next_probe_at = Instant::now() + ack_persist_interval(&ack);
                        if more_pending
                            && batch_last
                                .is_some_and(|timestamp| ack.persisted_through_ts >= timestamp)
                        {
                            next_probe_at = Instant::now();
                        }
                    }
                }
                // wait_for_live_ack consumes config and remote-task messages
                // internally, so only Ack/Closed/Pending reach this match; these
                // arms are kept defensively in case that contract changes.
                Ok(LiveRead::Config(config)) => {
                    if let Ok(mut target) = remote_config.lock() {
                        *target = Some(config);
                    }
                    drop_socket = true;
                }
                Ok(LiveRead::RemoteTask(_) | LiveRead::TaskResultAck(_)) => {}
                Ok(LiveRead::Pending) => {
                    eprintln!("live ACK timed out");
                    drop_socket = true;
                }
                Err(error) => {
                    eprintln!("live ACK read failed: {error}");
                    drop_socket = true;
                }
            }
        }
        if drop_socket {
            socket = None;
            accepted_through = persisted_through.load(Ordering::Acquire);
            next_send_at = Instant::now();
            next_probe_at = Instant::now();
            thread::sleep(backoff.next_delay());
        }
    }
}

