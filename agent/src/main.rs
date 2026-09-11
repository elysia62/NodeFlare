mod live;
mod live_batch;
mod metrics;
mod remote;
mod runtime_stats;
mod update;

// Re-exported so the rest of the crate keeps referring to these items by their
// original unqualified names (and so tests can keep using `crate::name`).
pub(crate) use live::*;
pub(crate) use metrics::*;
pub(crate) use remote::*;
pub(crate) use update::*;

use nodeflare_telemetry as telemetry;
use telemetry::{
    AGENT_CAPABILITIES_HEADER, AGENT_PROTOCOL_HEADER, AGENT_PROTOCOL_VERSION, DiskMetric,
    GpuMetric, LatencyResult, REQUIRED_AGENT_CAPABILITIES, Report,
};

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
use sysinfo::{Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System};
use tungstenite::client::IntoClientRequest;
use tungstenite::{Error as WebSocketError, Message, client_tls};

#[cfg(not(any(
    target_os = "linux",
    target_os = "windows",
    target_os = "macos",
    target_os = "freebsd"
)))]
compile_error!("nodeflare-agent supports Linux, Windows, macOS, and FreeBSD");

const VERSION: &str = match option_env!("NODEFLARE_VERSION") {
    Some(version) if !version.is_empty() => version,
    _ => env!("CARGO_PKG_VERSION"),
};
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/elysia62/NodeFlare/releases/latest";
const PROBE_ATTEMPTS: usize = 4;
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_AGENT_BINARY_BYTES: u64 = 64 * 1024 * 1024;
const REMOTE_TASK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REMOTE_STREAM_OUTPUT_BYTES: u64 = 512 * 1024;
const REMOTE_RESULT_OUTPUT_BYTES: usize = 900 * 1024;
const REMOTE_OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const REMOTE_RESULT_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const REMOTE_TASK_JOURNAL_VERSION: u32 = 1;
const MAX_REMOTE_TASK_JOURNAL_ENTRIES: usize = 32;
const MAX_REMOTE_TASK_JOURNAL_BYTES: u64 = 32 * 1024 * 1024;
const LATENCY_WORKERS: usize = 4;
const MAX_PENDING_LATENCY_RESULTS: usize = 4096;
const MAX_REPORT_AGE_SECONDS: i64 = 7_000;
const ONCE_WSS_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const UPDATE_RETRY_INTERVAL: Duration = Duration::from_secs(30 * 60);
const UPDATE_CHECK_JITTER_MAX_SECONDS: u64 = 30 * 60;
const LIVE_RECONNECT_DELAY: Duration = Duration::from_secs(3);
const LIVE_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(60);
const SPOOL_REWRITE_MIN_INTERVAL: Duration = Duration::from_secs(30);
const LIVE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LIVE_QUEUE_CAPACITY: usize = 720;
const MAX_PENDING_SPOOL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PENDING_SPOOL_LINE_BYTES: usize = 1024 * 1024;
const LIVE_ACK_READ_TIMEOUT: Duration = Duration::from_secs(5);
const LIVE_HINT_READ_TIMEOUT: Duration = Duration::from_millis(10);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const BASIC_INFO_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const SLOW_METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const GPU_METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const PUBLIC_IP_REFRESH_INTERVAL: Duration = Duration::from_secs(10 * 60);
const PUBLIC_IP_STALE_AFTER: Duration = Duration::from_secs(30 * 60);
const PUBLIC_IP_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const PUBLIC_IP_WAIT_BUDGET: Duration = Duration::from_secs(8);
const PUBLIC_IP_V4_URL: &str = "https://ipv4.icanhazip.com/";
const PUBLIC_IP_V6_URL: &str = "https://ipv6.icanhazip.com/";
const RUNTIME_STATS_INTERVAL: Duration = Duration::from_secs(5 * 60);
const CLOCK_CALIBRATION_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const CLOCK_CALIBRATION_MIN_CHANGE_MS: i64 = 20_000;

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
struct RuntimeConfig {
    token: String,
    endpoint: String,
    report_interval: u64,
    // Wire/config field name; controls upload batching, not the one-second sampler.
    collect_interval: u64,
    network_interface: String,
    agent_mirror: String,
    auto_update: bool,
    latency_tasks: Vec<LatencyTask>,
}

#[derive(Debug, Parser)]
#[command(name = "agent", version = VERSION, about = "NodeFlare monitoring agent")]
struct CliOptions {
    /// NodeFlare endpoint
    #[arg(short = 'e', value_name = "URL")]
    endpoint: String,
    /// Agent token
    #[arg(short = 't', value_name = "TOKEN", conflicts_with = "token_file")]
    token: Option<String>,
    /// Read the Agent token from a file instead of -t, which keeps it out of
    /// the process list
    #[arg(long = "token-file", value_name = "PATH")]
    token_file: Option<PathBuf>,
    /// Initial report interval in seconds (15-3600)
    #[arg(short = 'i', value_name = "SECONDS", default_value_t = 60)]
    interval: u64,
    /// Submit one report and exit
    #[arg(long, conflicts_with = "collect")]
    once: bool,
    /// Print one metric sample and exit
    #[arg(long, conflicts_with = "once")]
    collect: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LatencyTask {
    id: String,
    name: String,
    task_type: String,
    target: String,
    port: Option<i64>,
    interval_seconds: u64,
}

struct LatencyExecutor {
    task_tx: mpsc::Sender<LatencyTask>,
    result_rx: mpsc::Receiver<LatencyResult>,
    in_flight: HashSet<String>,
    stats: Arc<runtime_stats::RuntimeStats>,
}

impl LatencyExecutor {
    fn new(stats: Arc<runtime_stats::RuntimeStats>) -> Result<Self> {
        let (task_tx, task_rx) = mpsc::channel::<LatencyTask>();
        let (result_tx, result_rx) = mpsc::channel();
        let task_rx = Arc::new(Mutex::new(task_rx));

        for index in 0..LATENCY_WORKERS {
            let task_rx = Arc::clone(&task_rx);
            let result_tx = result_tx.clone();
            thread::Builder::new()
                .name(format!("nodeflare-latency-{index}"))
                .spawn(move || {
                    loop {
                        let task = match task_rx.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => return,
                        };
                        match task {
                            Ok(task) => {
                                if result_tx.send(execute_latency_task(task)).is_err() {
                                    return;
                                }
                            }
                            Err(mpsc::RecvError) => return,
                        };
                    }
                })?;
        }

        Ok(Self {
            task_tx,
            result_rx,
            in_flight: HashSet::new(),
            stats,
        })
    }

    fn enqueue(&mut self, task: LatencyTask) -> bool {
        if self.in_flight.contains(&task.id) {
            return false;
        }
        let task_id = task.id.clone();
        match self.task_tx.send(task) {
            Ok(()) => {
                self.in_flight.insert(task_id);
                true
            }
            Err(mpsc::SendError(_)) => {
                self.stats.latency_queue_rejected();
                false
            }
        }
    }

    fn drain(&mut self) -> Vec<LatencyResult> {
        let results = self.result_rx.try_iter().collect::<Vec<_>>();
        for result in &results {
            self.in_flight.remove(&result.task_id);
        }
        results
    }

    fn schedule(
        &mut self,
        tasks: &[LatencyTask],
        deadlines: &mut HashMap<String, Instant>,
        now: Instant,
    ) {
        for task in tasks {
            if deadlines
                .get(&task.id)
                .is_some_and(|deadline| *deadline > now)
            {
                continue;
            }
            let delay = if self.enqueue(task.clone()) {
                Duration::from_secs(task.interval_seconds.clamp(30, 3600))
            } else {
                // A queued/running task can outlive its interval. Never leave an
                // expired deadline behind for the main loop to spin on.
                SAMPLE_INTERVAL
            };
            deadlines.insert(task.id.clone(), now + delay);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteConfig {
    report_interval: u64,
    collect_interval: u64,
    network_interface: String,
    agent_mirror: String,
    auto_update: bool,
    latency_tasks: Vec<LatencyTask>,
}

#[derive(Debug, Default)]
struct ClockCalibration {
    offset_ms: i64,
    calibrated_at: Option<Instant>,
}

impl ClockCalibration {
    fn observe(&mut self, offset_ms: i64) -> bool {
        let should_update = self.calibrated_at.is_none()
            || self
                .calibrated_at
                .is_some_and(|at| at.elapsed() >= CLOCK_CALIBRATION_MAX_AGE)
            || self.offset_ms.abs_diff(offset_ms) >= CLOCK_CALIBRATION_MIN_CHANGE_MS as u64;
        if should_update {
            self.offset_ms = offset_ms;
            self.calibrated_at = Some(Instant::now());
        }
        should_update
    }
}

type SharedClock = Arc<Mutex<ClockCalibration>>;

#[derive(Default)]
pub(crate) struct PublicIpValue {
    address: Option<IpAddr>,
    last_success: Option<Instant>,
}

impl PublicIpValue {
    fn observe(&mut self, address: Option<IpAddr>, now: Instant) {
        if let Some(address) = address {
            self.address = Some(address);
            self.last_success = Some(now);
        } else if self
            .last_success
            .is_none_or(|at| now.saturating_duration_since(at) >= PUBLIC_IP_STALE_AFTER)
        {
            self.address = None;
        }
    }
}

#[derive(Default)]
pub(crate) struct PublicIps {
    v4: PublicIpValue,
    v6: PublicIpValue,
}

#[derive(Clone, Default)]
pub(crate) struct PublicIpProbe {
    state: Arc<Mutex<PublicIps>>,
    probing: Arc<AtomicBool>,
    last_attempt: Arc<Mutex<Option<Instant>>>,
}

impl PublicIpProbe {
    fn refresh(&self) {
        let due = self
            .last_attempt
            .lock()
            .unwrap()
            .is_none_or(|at| at.elapsed() >= PUBLIC_IP_REFRESH_INTERVAL);
        if !due || self.probing.swap(true, Ordering::SeqCst) {
            return;
        }
        *self.last_attempt.lock().unwrap() = Some(Instant::now());
        let state = Arc::clone(&self.state);
        let probing = Arc::clone(&self.probing);
        thread::spawn(move || {
            let v4 = probe_public_ip(PUBLIC_IP_V4_URL, false);
            let v6 = probe_public_ip(PUBLIC_IP_V6_URL, true);
            let observed_at = Instant::now();
            let mut state = state.lock().unwrap();
            state.v4.observe(v4, observed_at);
            state.v6.observe(v6, observed_at);
            drop(state);
            probing.store(false, Ordering::SeqCst);
        });
    }

    fn v4(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .v4
            .address
            .map(|ip| ip.to_string())
    }

    fn v6(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap()
            .v6
            .address
            .map(|ip| ip.to_string())
    }

    fn wait_initial(&self) {
        let deadline = Instant::now() + PUBLIC_IP_WAIT_BUDGET;
        while self.probing.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(150));
        }
    }
}

fn probe_public_ip(url: &str, ipv6: bool) -> Option<IpAddr> {
    let probe_agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(PUBLIC_IP_PROBE_TIMEOUT))
        .build()
        .into();
    let response = probe_agent
        .get(url)
        .header("User-Agent", format!("nodeflare-agent/{VERSION}"))
        .call()
        .ok()?;
    let mut body = String::new();
    response
        .into_body()
        .into_reader()
        .take(128)
        .read_to_string(&mut body)
        .ok()?;
    parse_public_ip(&body, ipv6)
}

fn parse_public_ip(text: &str, ipv6: bool) -> Option<IpAddr> {
    let ip: IpAddr = text.trim().parse().ok()?;
    match (ipv6, ip) {
        (true, IpAddr::V6(_)) | (false, IpAddr::V4(_)) => Some(ip),
        _ => None,
    }
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn unix_timestamp_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn clock_offset_from_http_date(value: &str, started_ms: i64, ended_ms: i64) -> Option<i64> {
    let server_ms: i64 = httpdate::parse_http_date(value)
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis()
        .try_into()
        .ok()?;
    let midpoint_ms = started_ms.saturating_add(ended_ms.saturating_sub(started_ms) / 2);
    Some(server_ms.saturating_sub(midpoint_ms))
}

fn corrected_timestamp(timestamp: i64, offset_ms: i64) -> i64 {
    timestamp
        .saturating_mul(1_000)
        .saturating_add(offset_ms)
        .div_euclid(1_000)
}

fn monotonic_report_timestamp(corrected: i64, last_emitted: i64, persisted_through: i64) -> i64 {
    corrected
        .max(last_emitted.saturating_add(1))
        .max(persisted_through.saturating_add(1))
}

fn shared_clock_offset(clock: &SharedClock) -> i64 {
    clock.lock().map_or(0, |calibration| calibration.offset_ms)
}

fn observe_clock(clock: &SharedClock, offset_ms: Option<i64>) {
    let Some(offset_ms) = offset_ms else {
        return;
    };
    if let Ok(mut calibration) = clock.lock() {
        calibration.observe(offset_ms);
    }
}

fn valid_sample_schedule(report_interval: u64, collect_interval: u64) -> bool {
    (15..=3600).contains(&report_interval)
        && (1..=60).contains(&collect_interval)
        && collect_interval <= report_interval
        && report_interval.div_ceil(collect_interval) <= LIVE_QUEUE_CAPACITY as u64
}

fn advance_deadline(mut deadline: Instant, interval: Duration, now: Instant) -> Instant {
    while deadline <= now {
        deadline += interval;
    }
    deadline
}

/// Reads an Agent token from a file, trimming surrounding whitespace (editors
/// and shell redirection usually append a trailing newline).
fn read_token_file(path: &Path) -> Result<String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("failed to read token file {}: {error}", path.display()))?;
    Ok(contents.trim().to_string())
}

fn runtime_config(options: &CliOptions) -> Result<RuntimeConfig> {
    let interval = options.interval;
    if !(15..=3600).contains(&interval) {
        return Err("interval must be between 15 and 3600 seconds".into());
    }
    let token = match &options.token {
        Some(token) => token.clone(),
        None => match options.token_file.as_deref() {
            Some(path) => read_token_file(path)?,
            None => env::var("NODEFLARE_AGENT_TOKEN").unwrap_or_default(),
        },
    };
    let endpoint = options.endpoint.clone();
    if token.is_empty() || token.len() > 512 || token.chars().any(char::is_whitespace) {
        return Err("token is invalid".into());
    }
    if !valid_endpoint(&endpoint) {
        return Err(
            "endpoint must use HTTPS; HTTP is only allowed for loopback development".into(),
        );
    }
    Ok(RuntimeConfig {
        token,
        endpoint: endpoint.trim_end_matches('/').to_string(),
        report_interval: interval,
        collect_interval: telemetry::MIN_UPLOAD_INTERVAL,
        network_interface: String::new(),
        agent_mirror: String::new(),
        auto_update: true,
        latency_tasks: Vec::new(),
    })
}


fn flush_remote_results(socket: &mut LiveSocket, executor: &mut RemoteExecutor) -> Result<()> {
    for result in executor.due_results() {
        socket.send(Message::Text(serde_json::to_string(&result)?.into()))?;
        executor.mark_sent(&result.task_id);
    }
    Ok(())
}

fn apply_remote(config: &mut RuntimeConfig, remote: &RemoteConfig) -> bool {
    if valid_sample_schedule(remote.report_interval, remote.collect_interval) {
        config.report_interval = remote.report_interval;
        config.collect_interval = remote.collect_interval.max(telemetry::MIN_UPLOAD_INTERVAL);
    }
    config
        .network_interface
        .clone_from(&remote.network_interface);
    let mirror = remote.agent_mirror.trim().trim_end_matches('/');
    if mirror.is_empty() || valid_endpoint(mirror) {
        config.agent_mirror = mirror.to_string();
    }
    config.auto_update = remote.auto_update;
    let tasks = sanitize_latency_tasks(&remote.latency_tasks);
    let changed = config.latency_tasks != tasks;
    config.latency_tasks = tasks;
    changed
}

fn sanitize_latency_tasks(tasks: &[LatencyTask]) -> Vec<LatencyTask> {
    let mut seen = HashSet::new();
    tasks
        .iter()
        .filter(|task| {
            !task.id.is_empty()
                && task.id.len() <= 80
                && (1..=80).contains(&task.name.trim().chars().count())
                && (30..=3600).contains(&task.interval_seconds)
                && matches!(task.task_type.as_str(), "tcp" | "icmp")
                && ((task.task_type == "tcp"
                    && task.port.is_some_and(|port| (1..=65535).contains(&port)))
                    || (task.task_type == "icmp" && task.port.is_none()))
                && parse_probe_target(&task.target, None).is_some()
                && seen.insert(task.id.clone())
        })
        .cloned()
        .collect()
}

fn valid_endpoint(value: &str) -> bool {
    let value = value.trim_end_matches('/');
    if value.is_empty() || value.len() > 2048 || value.chars().any(char::is_whitespace) {
        return false;
    }
    let Ok(parsed) = url::Url::parse(value) else {
        return false;
    };
    if parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return false;
    }
    if parsed.scheme() == "https" {
        return true;
    }
    if parsed.scheme() != "http" {
        return false;
    }
    match parsed.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn update_check_jitter(token: &str) -> Duration {
    let digest = Sha256::digest(token.as_bytes());
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    Duration::from_secs(
        u64::from_be_bytes(bytes) % UPDATE_CHECK_JITTER_MAX_SECONDS.saturating_add(1),
    )
}

fn prune_report_samples(samples: &mut Vec<Report>, now: i64) {
    samples.retain(|report| (report.timestamp - now).abs() <= MAX_REPORT_AGE_SECONDS);
    let mut remaining = MAX_PENDING_LATENCY_RESULTS;
    for report in samples.iter_mut().rev() {
        if report.latency_results.len() > remaining {
            let discard = report.latency_results.len() - remaining;
            report.latency_results.drain(..discard);
            remaining = 0;
        } else {
            remaining -= report.latency_results.len();
        }
    }
}

fn agent_state_directory() -> Result<PathBuf> {
    if let Some(value) = env::var_os("NODEFLARE_STATE_DIR") {
        let directory = PathBuf::from(value);
        if !directory.is_absolute() {
            return Err("NODEFLARE_STATE_DIR must be an absolute path".into());
        }
        fs::create_dir_all(&directory)?;
        return Ok(directory);
    }
    #[cfg(target_os = "windows")]
    {
        let directory = env::var_os("ProgramData")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.join("NodeFlare").join("Agent"))
            .unwrap_or_else(|| {
                env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("data")
                    .join("agent")
            });
        fs::create_dir_all(&directory)?;
        return Ok(directory);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let executable = env::current_exe()?;
        let directory = executable
            .parent()
            .ok_or("agent executable path has no parent directory")?;
        Ok(directory.to_path_buf())
    }
}

fn pending_spool_path() -> Result<PathBuf> {
    Ok(agent_state_directory()?.join("pending.jsonl"))
}

fn remote_task_journal_path() -> Result<PathBuf> {
    Ok(agent_state_directory()?.join("remote-tasks.json"))
}

fn load_pending_spool(path: &Path) -> Result<Vec<Report>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() > MAX_PENDING_SPOOL_BYTES {
        return Err("pending report spool is too large".into());
    }
    let mut samples = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.len() > MAX_PENDING_SPOOL_LINE_BYTES {
            continue;
        }
        if let Ok(report) = serde_json::from_str::<Report>(&line) {
            samples.push(report);
        }
    }
    if samples.len() > LIVE_QUEUE_CAPACITY {
        let overflow = samples.len() - LIVE_QUEUE_CAPACITY;
        samples.drain(..overflow);
    }
    Ok(samples)
}

fn append_pending_spool(path: &Path, report: &Report) -> Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    serde_json::to_writer(&mut file, report)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn rewrite_pending_spool(path: &Path, samples: &[Report]) -> Result<()> {
    if samples.is_empty() {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
    }
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("pending"),
        std::process::id()
    ));
    let mut file = fs::File::create(&temporary)?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    for report in samples {
        serde_json::to_writer(&mut file, report)?;
        file.write_all(b"\n")?;
    }
    file.sync_data()?;
    // fs::rename replaces the destination on Windows too, so no pre-delete is
    // needed here (same reasoning as write_remote_task_journal).
    fs::rename(&temporary, path)?;
    Ok(())
}

/// Compacts the spool at most once per `SPOOL_REWRITE_MIN_INTERVAL`.
///
/// While the live connection is down and the queue sits at capacity the sample
/// set changes every second; rewriting and fsyncing the full spool that often
/// wears the disk for no benefit. The in-memory queue plus the per-sample line
/// appends stay authoritative until the next due rewrite.
fn rewrite_spool_if_due(path: &Path, samples: &[Report], last: &mut Option<Instant>) {
    if last.is_some_and(|previous| previous.elapsed() < SPOOL_REWRITE_MIN_INTERVAL) {
        return;
    }
    match rewrite_pending_spool(path, samples) {
        Ok(()) => *last = Some(Instant::now()),
        Err(error) => eprintln!("pending report spool compaction failed: {error}"),
    }
}


fn run(options: &CliOptions, once: bool, print_only: bool) -> Result<()> {
    let mut config = runtime_config(options)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let mut collector = Collector::new(&config);
    collector.public_ip.refresh();
    if once || print_only {
        collector.public_ip.wait_initial();
    }
    let mut next_collect = Instant::now() + SAMPLE_INTERVAL;
    let mut next_latency: HashMap<String, Instant> = HashMap::new();
    let stats = Arc::new(runtime_stats::RuntimeStats::default());
    let mut latency_executor = LatencyExecutor::new(Arc::clone(&stats))?;
    let mut pending_results = Vec::new();
    let spool_path = pending_spool_path()?;
    let mut last_spool_rewrite: Option<Instant> = None;
    let mut pending_samples = load_pending_spool(&spool_path).unwrap_or_else(|error| {
        eprintln!("pending report spool ignored: {error}");
        Vec::new()
    });
    let loaded_len = pending_samples.len();
    let loaded_latency = pending_samples
        .iter()
        .map(|report| report.latency_results.len())
        .sum::<usize>();
    prune_report_samples(&mut pending_samples, unix_timestamp());
    if (loaded_len != pending_samples.len()
        || loaded_latency
            != pending_samples
                .iter()
                .map(|report| report.latency_results.len())
                .sum::<usize>())
        && let Err(error) = rewrite_pending_spool(&spool_path, &pending_samples)
    {
        eprintln!("pending report spool cleanup failed: {error}");
    }
    let mut last_emitted_timestamp = pending_samples
        .iter()
        .map(|report| report.timestamp)
        .max()
        .unwrap_or(0);
    let mut next_update_check = Instant::now() + update_check_jitter(&config.token);
    let mut update_checkpoint = None;
    let mut next_stats_log = Instant::now() + RUNTIME_STATS_INTERVAL;
    let clock = Arc::new(Mutex::new(ClockCalibration::default()));
    let live = (!print_only)
        .then(|| LiveSender::start(&config, Arc::clone(&clock), Arc::clone(&stats)))
        .transpose()?;
    if let Some(live) = &live {
        for report in &pending_samples {
            live.send(report);
        }
    }
    let once_deadline = (once && !print_only).then(|| Instant::now() + ONCE_WSS_TIMEOUT);
    let mut once_target_timestamp = None;
    loop {
        if let Some(live) = &live {
            let persisted_through = live.persisted_through();
            let before = pending_samples.len();
            pending_samples.retain(|report| report.timestamp > persisted_through);
            stats.persisted_samples_pruned(before.saturating_sub(pending_samples.len()));
            if before != pending_samples.len() {
                rewrite_spool_if_due(&spool_path, &pending_samples, &mut last_spool_rewrite);
            }
            if once_target_timestamp.is_some_and(|target| persisted_through >= target) {
                return Ok(());
            }
            if let Some(remote) = live.take_remote_config() {
                let previous_collect_interval = config.collect_interval;
                let previous_report_interval = config.report_interval;
                if apply_remote(&mut config, &remote) {
                    next_latency.clear();
                }
                if config.collect_interval != previous_collect_interval
                    || config.report_interval != previous_report_interval
                {
                    live.set_send_interval(config.collect_interval);
                }
            }
        }
        if once_target_timestamp.is_some()
            && once_deadline.is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err("WSS report persistence acknowledgement timed out".into());
        }

        // Continuous sampling need not leave the queue empty. Wait for the samples
        // present when the check became due, and preserve newer ones across restart.
        if config.auto_update && Instant::now() >= next_update_check {
            update_checkpoint.get_or_insert(last_emitted_timestamp);
        }
        if !once
            && !print_only
            && config.auto_update
            && update_checkpoint.is_some_and(|timestamp| {
                live.as_ref()
                    .is_some_and(|sender| sender.persisted_through() >= timestamp)
            })
        {
            update_checkpoint = None;
            match rewrite_pending_spool(&spool_path, &pending_samples)
                .and_then(|()| update(&agent, &config.agent_mirror))
            {
                Ok(true) => return Ok(()),
                Ok(false) => next_update_check = Instant::now() + UPDATE_CHECK_INTERVAL,
                Err(error) => {
                    eprintln!("agent update failed: {error}");
                    next_update_check = Instant::now() + UPDATE_RETRY_INTERVAL;
                }
            }
        }
        pending_results.extend(latency_executor.drain());
        latency_executor.schedule(&config.latency_tasks, &mut next_latency, Instant::now());

        if Instant::now() >= next_collect && (!once || once_target_timestamp.is_none()) {
            let offset_ms = shared_clock_offset(&clock);
            let mut latest_results = HashMap::new();
            for mut result in std::mem::take(&mut pending_results) {
                result.timestamp = corrected_timestamp(result.timestamp, offset_ms);
                latest_results.insert(result.task_id.clone(), result);
            }
            // CPU and network counters are read at the beginning of collect.
            // A slow GPU/disk refresh must not shift their timestamp to a later second.
            let timestamp = monotonic_report_timestamp(
                corrected_timestamp(unix_timestamp(), offset_ms),
                last_emitted_timestamp,
                live.as_ref().map_or(0, LiveSender::persisted_through),
            );
            let collection_started = Instant::now();
            let report =
                collector.collect(&config, latest_results.into_values().collect(), timestamp);
            stats.collection_finished(collection_started.elapsed());
            last_emitted_timestamp = report.timestamp;
            if print_only {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            if let Err(error) = append_pending_spool(&spool_path, &report) {
                // A spool write failure (disk full, transient IO error) must not
                // kill the agent: the sample is still queued in memory and will
                // be retried on the next compaction.
                eprintln!("pending report spool append failed: {error}");
            }
            if let Some(live) = &live {
                live.send(&report);
            }
            if once {
                once_target_timestamp = Some(report.timestamp);
            }
            pending_samples.push(report);
            let before_prune = pending_samples.len();
            let latency_before_prune = pending_samples
                .iter()
                .map(|report| report.latency_results.len())
                .sum::<usize>();
            prune_report_samples(
                &mut pending_samples,
                corrected_timestamp(unix_timestamp(), offset_ms),
            );
            if pending_samples.len() > LIVE_QUEUE_CAPACITY {
                let overflow = pending_samples.len() - LIVE_QUEUE_CAPACITY;
                pending_samples.drain(..overflow);
                stats.live_queue_dropped(overflow);
            }
            if before_prune != pending_samples.len()
                || latency_before_prune
                    != pending_samples
                        .iter()
                        .map(|report| report.latency_results.len())
                        .sum::<usize>()
            {
                rewrite_spool_if_due(&spool_path, &pending_samples, &mut last_spool_rewrite);
            }
            next_collect = advance_deadline(next_collect, SAMPLE_INTERVAL, Instant::now());
        }

        if Instant::now() >= next_stats_log {
            stats.log_and_reset();
            next_stats_log = Instant::now() + RUNTIME_STATS_INTERVAL;
        }

        let collect_wake_at = if once_target_timestamp.is_some() {
            once_deadline.unwrap_or(next_collect)
        } else {
            next_collect
        };
        let maintenance_wake_at = if config.auto_update && update_checkpoint.is_none() {
            next_stats_log.min(next_update_check)
        } else {
            next_stats_log
        };
        let wake_at = next_latency
            .values()
            .copied()
            .min()
            .map_or(collect_wake_at, |deadline| deadline.min(collect_wake_at))
            .min(maintenance_wake_at);
        let wait = wake_at
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(1));
        if !wait.is_zero() {
            thread::sleep(wait);
        }
    }
}

fn main() {
    let options = CliOptions::parse();
    let result = run(&options, options.once || options.collect, options.collect);
    if let Err(error) = result {
        eprintln!("agent: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;

