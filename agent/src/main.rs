mod live_batch;
mod runtime_stats;
use nodeflare_telemetry as telemetry;
use telemetry::{DiskMetric, GpuMetric, LatencyResult, Report};

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
const LIVE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LIVE_QUEUE_CAPACITY: usize = 720;
const MAX_PENDING_SPOOL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PENDING_SPOOL_LINE_BYTES: usize = 1024 * 1024;
const LIVE_ACK_READ_TIMEOUT: Duration = Duration::from_secs(5);
const LIVE_HINT_READ_TIMEOUT: Duration = Duration::from_millis(10);
const BASIC_INFO_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const SLOW_METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
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
    #[arg(short = 't', value_name = "TOKEN")]
    token: Option<String>,
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

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
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
struct PublicIpValue {
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
struct PublicIps {
    v4: PublicIpValue,
    v6: PublicIpValue,
}

#[derive(Clone, Default)]
struct PublicIpProbe {
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

struct LiveSender {
    pending: Arc<(Mutex<VecDeque<Report>>, Condvar)>,
    send_interval: Arc<Mutex<Duration>>,
    persisted_through: Arc<AtomicI64>,
    remote_config: Arc<Mutex<Option<RemoteConfig>>>,
    stats: Arc<runtime_stats::RuntimeStats>,
}

struct LiveSenderWorker {
    pending: Arc<(Mutex<VecDeque<Report>>, Condvar)>,
    configured_interval: Arc<Mutex<Duration>>,
    persisted_through: Arc<AtomicI64>,
    remote_config: Arc<Mutex<Option<RemoteConfig>>>,
    clock: SharedClock,
    stats: Arc<runtime_stats::RuntimeStats>,
}

impl LiveSender {
    fn start(
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

    fn send(&self, report: &Report) {
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

    fn set_send_interval(&self, collect_interval: u64) {
        if let Ok(mut interval) = self.send_interval.lock() {
            *interval = live_batch_interval(collect_interval);
        }
        self.pending.1.notify_one();
    }

    fn persisted_through(&self) -> i64 {
        self.persisted_through.load(Ordering::Acquire)
    }

    fn take_remote_config(&self) -> Option<RemoteConfig> {
        self.remote_config
            .lock()
            .ok()
            .and_then(|mut config| config.take())
    }
}

fn live_batch_interval(collect_interval: u64) -> Duration {
    Duration::from_secs(collect_interval.clamp(telemetry::MIN_COLLECT_INTERVAL, 60))
}

#[derive(Debug, Default, Clone)]
struct BasicMetrics {
    cpu_cores: i64,
    cpu_model: String,
    os: String,
    kernel: String,
    arch: String,
    virtualization: String,
    gpu_usage: f64,
    gpu_model: String,
    gpus: Vec<GpuMetric>,
}

#[derive(Debug, Default, Clone)]
struct SlowMetrics {
    disks: Vec<DiskMetric>,
    processes: i64,
    tcp_connections: i64,
    udp_connections: i64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy)]
struct CpuSample {
    total: u64,
    idle: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy)]
struct IoSample {
    rx: u64,
    tx: u64,
    read_ops: u64,
    read_sectors: u64,
    write_ops: u64,
    write_sectors: u64,
    read_millis: u64,
    write_millis: u64,
    io_millis: u64,
}

#[cfg(target_os = "linux")]
fn text(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn command(name: &str, args: &[&str]) -> String {
    Command::new(name)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn cpu_sample() -> CpuSample {
    let line = text("/proc/stat").lines().next().unwrap_or("").to_string();
    let values = line
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .collect::<Vec<_>>();
    CpuSample {
        total: values.iter().sum(),
        idle: values.get(3).copied().unwrap_or(0) + values.get(4).copied().unwrap_or(0),
    }
}

fn selected_interface(name: &str, filter: &str) -> bool {
    if filter.trim().is_empty() {
        name != "lo" && !name.to_ascii_lowercase().contains("loopback")
    } else {
        filter.split(',').any(|value| value.trim() == name)
    }
}

#[cfg(target_os = "linux")]
fn disk_device(name: &str) -> bool {
    ((name.starts_with("sd") || name.starts_with("vd") || name.starts_with("xvd"))
        && name
            .chars()
            .last()
            .is_some_and(|value| value.is_ascii_alphabetic()))
        || (name.starts_with("nvme") && name.contains('n') && !name.contains('p'))
        || (name.starts_with("mmcblk") && !name.contains('p'))
}

#[cfg(target_os = "linux")]
fn io_sample(filter: &str) -> IoSample {
    let mut sample = IoSample::default();
    for line in text("/proc/net/dev")
        .lines()
        .filter(|line| line.contains(':'))
    {
        let Some((name, values)) = line.split_once(':') else {
            continue;
        };
        if !selected_interface(name.trim(), filter) {
            continue;
        }
        let fields = values.split_whitespace().collect::<Vec<_>>();
        sample.rx = sample
            .rx
            .saturating_add(fields.first().and_then(|v| v.parse().ok()).unwrap_or(0));
        sample.tx = sample
            .tx
            .saturating_add(fields.get(8).and_then(|v| v.parse().ok()).unwrap_or(0));
    }
    for line in text("/proc/diskstats").lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 13 || !disk_device(fields[2]) {
            continue;
        }
        sample.read_ops = sample
            .read_ops
            .saturating_add(fields[3].parse().unwrap_or(0));
        sample.read_sectors = sample
            .read_sectors
            .saturating_add(fields[5].parse().unwrap_or(0));
        sample.write_ops = sample
            .write_ops
            .saturating_add(fields[7].parse().unwrap_or(0));
        sample.write_sectors = sample
            .write_sectors
            .saturating_add(fields[9].parse().unwrap_or(0));
        sample.read_millis = sample
            .read_millis
            .saturating_add(fields[6].parse().unwrap_or(0));
        sample.write_millis = sample
            .write_millis
            .saturating_add(fields[10].parse().unwrap_or(0));
        sample.io_millis = sample
            .io_millis
            .saturating_add(fields[12].parse().unwrap_or(0));
    }
    sample
}

#[cfg(target_os = "linux")]
fn mem_value(contents: &str, key: &str) -> i64 {
    contents
        .lines()
        .find(|line| line.starts_with(key))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
        .saturating_mul(1024)
}

#[cfg(target_os = "linux")]
fn file_line_count(path: &str) -> i64 {
    text(path).lines().skip(1).count() as i64
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
fn connection_counts_from_netstat(output: &str) -> (i64, i64) {
    output.lines().fold((0_i64, 0_i64), |(tcp, udp), line| {
        let protocol = line
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if protocol.starts_with("tcp") {
            (tcp.saturating_add(1), udp)
        } else if protocol.starts_with("udp") {
            (tcp, udp.saturating_add(1))
        } else {
            (tcp, udp)
        }
    })
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
fn connection_counts() -> (i64, i64) {
    Command::new("netstat")
        .args(["-an"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| connection_counts_from_netstat(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn disk_usage() -> Vec<DiskMetric> {
    let output = command(
        "df",
        &[
            "-B1",
            "-l",
            "--output=source,target,size,used",
            "-x",
            "tmpfs",
            "-x",
            "devtmpfs",
        ],
    );
    output
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 4 {
                return None;
            }
            let total = fields[2].parse::<i64>().ok()?;
            let used = fields[3].parse::<i64>().ok()?;
            (total > 0).then(|| DiskMetric {
                name: fields[0].to_string(),
                mount_point: fields[1].to_string(),
                used,
                total,
                ..DiskMetric::default()
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn os_name() -> String {
    text("/etc/os-release")
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map_or_else(
            || command("uname", &["-s"]),
            |value| value.trim_matches('"').to_string(),
        )
}

#[cfg(target_os = "linux")]
fn cpu_model() -> String {
    text("/proc/cpuinfo")
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            matches!(key.trim(), "model name" | "Hardware").then(|| value.trim().to_string())
        })
        .unwrap_or_default()
}

fn valid_probe_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 50 || host.starts_with('.') || host.ends_with('.') {
        return false;
    }
    let labels = host.split('.').collect::<Vec<_>>();
    let ipv4_like = labels.len() == 4
        && labels
            .iter()
            .all(|label| !label.is_empty() && label.chars().all(|c| c.is_ascii_digit()));
    if ipv4_like {
        return host
            .parse()
            .is_ok_and(|address| is_public_probe_ip(IpAddr::V4(address)));
    }
    let lower = host.to_ascii_lowercase();
    if labels.len() < 2
        || ["local", "localhost", "internal", "lan", "localdomain"]
            .iter()
            .any(|suffix| lower == *suffix || lower.ends_with(&format!(".{suffix}")))
        || lower == "home.arpa"
        || lower.ends_with(".home.arpa")
    {
        return false;
    }
    labels.iter().all(|label| {
        !label.is_empty()
            && label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
            && label
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric())
            && label
                .chars()
                .last()
                .is_some_and(|character| character.is_ascii_alphanumeric())
    })
}

fn is_public_probe_ip(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            let [a, b, c, _] = address.octets();
            !(a == 0
                || a == 10
                || a == 127
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 192 && b == 0 && c == 2)
                || (a == 192 && b == 168)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113)
                || a >= 224)
        }
        IpAddr::V6(address) => {
            if let Some(mapped) = address.to_ipv4_mapped() {
                return is_public_probe_ip(IpAddr::V4(mapped));
            }
            address.segments()[0] & 0xe000 == 0x2000
        }
    }
}

fn resolve_public_probe_address(host: &str, port: u16) -> Option<SocketAddr> {
    let addresses = (host, port)
        .to_socket_addrs()
        .ok()?
        .filter(|address| is_public_probe_ip(address.ip()))
        .collect::<Vec<_>>();
    addresses
        .iter()
        .find(|address| address.is_ipv4())
        .or_else(|| addresses.first())
        .copied()
}

fn parse_probe_target(value: &str, port: Option<u16>) -> Option<(String, u16)> {
    let raw = value.trim();
    if raw.is_empty()
        || raw.len() > 60
        || raw.contains("://")
        || raw
            .chars()
            .any(|character| character.is_whitespace() || "/@?#\\[]".contains(character))
        || raw.contains(':')
    {
        return None;
    }
    let port = port.unwrap_or(443);
    (port > 0 && valid_probe_host(raw)).then(|| (raw.to_ascii_lowercase(), port))
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

fn tcp_latency_probe(target: &str, port: Option<i64>) -> (f64, f64) {
    if target.trim().is_empty() {
        return (-1.0, -1.0);
    }
    let Some(port) = port.and_then(|value| u16::try_from(value).ok()) else {
        return (-1.0, 100.0);
    };
    let Some((host, port)) = parse_probe_target(target, Some(port)) else {
        return (-1.0, 100.0);
    };
    let Some(address) = resolve_public_probe_address(&host, port) else {
        return (-1.0, 100.0);
    };
    tcp_latency_probe_address(&address)
}

fn tcp_latency_probe_address(address: &SocketAddr) -> (f64, f64) {
    let mut latencies = Vec::with_capacity(PROBE_ATTEMPTS);
    for _ in 0..PROBE_ATTEMPTS {
        let started = Instant::now();
        if TcpStream::connect_timeout(address, PROBE_TIMEOUT).is_ok() {
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let loss = (PROBE_ATTEMPTS - latencies.len()) as f64 * 100.0 / PROBE_ATTEMPTS as f64;
    if latencies.is_empty() {
        (-1.0, loss)
    } else {
        (median(&mut latencies), loss)
    }
}

fn ping_latency(output: &str) -> Option<f64> {
    let marker = output
        .find("time=")
        .map(|index| (index + 5, false))
        .or_else(|| output.find("time<").map(|index| (index + 5, true)))?;
    if marker.1 {
        return Some(0.5);
    }
    let value = output[marker.0..]
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect::<String>();
    value.parse().ok()
}

fn ping_latencies(output: &str) -> Vec<f64> {
    output.lines().filter_map(ping_latency).collect()
}

fn icmp_latency_probe(target: &str) -> (f64, f64) {
    let host = target.trim();
    let Some((host, _)) = parse_probe_target(host, None) else {
        return (-1.0, 100.0);
    };
    let Some(address) = resolve_public_probe_address(&host, 443) else {
        return (-1.0, 100.0);
    };
    let destination = address.ip().to_string();
    let mut ping = Command::new("ping");
    #[cfg(target_os = "linux")]
    ping.args(["-n", "-c", "4", "-W", "1", destination.as_str()]);
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    ping.args(["-n", "-c", "4", "-W", "1000", destination.as_str()]);
    #[cfg(target_os = "windows")]
    ping.args(["-n", "4", "-w", "1000", destination.as_str()]);
    let mut latencies = ping
        .output()
        .map(|output| ping_latencies(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default();
    latencies.truncate(PROBE_ATTEMPTS);
    let loss = (PROBE_ATTEMPTS - latencies.len()) as f64 * 100.0 / PROBE_ATTEMPTS as f64;
    if latencies.is_empty() {
        (-1.0, loss)
    } else {
        (median(&mut latencies), loss)
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

fn execute_latency_task(task: LatencyTask) -> LatencyResult {
    let (latency_ms, packet_loss) = match task.task_type.as_str() {
        "tcp" => tcp_latency_probe(&task.target, task.port),
        "icmp" => icmp_latency_probe(&task.target),
        _ => (-1.0, 100.0),
    };
    LatencyResult {
        task_id: task.id,
        timestamp: unix_timestamp(),
        latency_ms,
        packet_loss,
    }
}

fn nvidia_gpu_info() -> Vec<GpuMetric> {
    let gpu = command(
        "nvidia-smi",
        &[
            "--query-gpu=utilization.gpu,name,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ],
    );
    gpu.lines()
        .filter_map(|line| {
            let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
            if fields.len() < 4 {
                return None;
            }
            if fields[1].is_empty() {
                return None;
            }
            Some(GpuMetric {
                usage: fields[0].parse().ok(),
                model: fields[1].to_string(),
                memory_used: fields[2]
                    .parse::<i64>()
                    .unwrap_or(0)
                    .saturating_mul(1024 * 1024),
                memory_total: fields[3]
                    .parse::<i64>()
                    .unwrap_or(0)
                    .saturating_mul(1024 * 1024),
            })
        })
        .collect()
}

fn basic_gpu_metrics(names: impl IntoIterator<Item = String>) -> Vec<GpuMetric> {
    let mut seen = HashSet::new();
    names
        .into_iter()
        .filter_map(|name| {
            let name = name.split(" (rev ").next().unwrap_or(&name).trim();
            let lower = name.to_ascii_lowercase();
            if name.is_empty()
                || [
                    "sensor hub",
                    "management engine",
                    "ethernet",
                    "wireless",
                    "audio controller",
                    "usb controller",
                    "sata controller",
                    "virtio",
                    "vmware",
                    "qxl",
                    "hyper-v",
                    "cirrus",
                    "microsoft basic display",
                ]
                .iter()
                .any(|pattern| lower.contains(pattern))
                || !seen.insert(name.to_string())
            {
                return None;
            }
            Some(GpuMetric {
                model: name.to_string(),
                usage: None,
                memory_used: 0,
                memory_total: 0,
            })
        })
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn parse_lspci_gpu_names(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("vga compatible controller")
                || lower.contains("3d controller")
                || lower.contains("display controller")
        })
        .filter_map(|line| line.split_once(": ").map(|(_, name)| name.to_string()))
        .collect()
}

#[cfg(any(target_os = "linux", test))]
fn gpu_name_from_uevent(output: &str) -> Option<String> {
    let driver = output
        .lines()
        .find_map(|line| line.strip_prefix("DRIVER="))?;
    let pci_class = output
        .lines()
        .find_map(|line| line.strip_prefix("PCI_CLASS="))
        .unwrap_or_default();
    if u32::from_str_radix(pci_class, 16)
        .ok()
        .map(|class| class >> 16)
        != Some(3)
    {
        return None;
    }
    match driver {
        "i915" | "xe" => Some("Intel Integrated Graphics".to_string()),
        "amdgpu" | "radeon" => Some("AMD Radeon Graphics".to_string()),
        "nvidia" | "nouveau" => Some("NVIDIA GPU".to_string()),
        "msm" | "msm_drm" => Some("Qualcomm Adreno GPU".to_string()),
        "panfrost" | "lima" => Some("ARM Mali GPU".to_string()),
        "vc4" | "v3d" => Some("Broadcom VideoCore Graphics".to_string()),
        "virtio-pci" | "virtio_gpu" | "bochs-drm" | "qxl" | "vmwgfx" | "cirrus" | "vboxvideo"
        | "hyperv_fb" | "simpledrm" | "simplefb" => None,
        other if !other.is_empty() => Some(format!("GPU ({other})")),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
fn sysfs_gpu_names() -> Vec<String> {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return Vec::new();
    };
    entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("card").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
            })
        })
        .filter_map(|entry| gpu_name_from_uevent(&text(entry.path().join("device/uevent"))))
        .collect()
}

#[cfg(any(target_os = "macos", test))]
fn parse_system_profiler_gpu_names(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("Chipset Model:")
                .map(|name| name.trim().to_string())
        })
        .collect()
}

#[cfg(any(target_os = "freebsd", test))]
fn parse_pciconf_gpu_names(output: &str) -> Vec<String> {
    fn block_name(lines: &[&str]) -> Option<String> {
        let header = lines.first()?.to_ascii_lowercase();
        let mut display = header.contains("class=0x03");
        let mut vendor = None;
        let mut device = None;
        for line in lines.iter().skip(1) {
            let Some((key, value)) = line.trim().split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim().trim_matches(['\'', '"']);
            match key {
                "class" if value.eq_ignore_ascii_case("display") => display = true,
                "vendor" => vendor = Some(value),
                "device" => device = Some(value),
                _ => {}
            }
        }
        if !display {
            return None;
        }
        match (vendor, device) {
            (Some(vendor), Some(device))
                if !device
                    .to_ascii_lowercase()
                    .contains(&vendor.to_ascii_lowercase()) =>
            {
                Some(format!("{vendor} {device}"))
            }
            (_, Some(device)) => Some(device.to_string()),
            (Some(vendor), None) => Some(vendor.to_string()),
            _ => None,
        }
    }

    let mut names = Vec::new();
    let mut block = Vec::new();
    for line in output.lines() {
        if !line.is_empty()
            && !line.chars().next().is_some_and(char::is_whitespace)
            && !block.is_empty()
        {
            if let Some(name) = block_name(&block) {
                names.push(name);
            }
            block.clear();
        }
        if !line.trim().is_empty() {
            block.push(line);
        }
    }
    if let Some(name) = block_name(&block) {
        names.push(name);
    }
    names
}

#[cfg(target_os = "linux")]
fn basic_gpu_info() -> Vec<GpuMetric> {
    let names = parse_lspci_gpu_names(&command("lspci", &[]));
    basic_gpu_metrics(if names.is_empty() {
        sysfs_gpu_names()
    } else {
        names
    })
}

#[cfg(target_os = "windows")]
fn basic_gpu_info() -> Vec<GpuMetric> {
    basic_gpu_metrics(
        command(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_VideoController | ForEach-Object { $_.Name }",
            ],
        )
        .lines()
        .map(str::to_string),
    )
}

#[cfg(target_os = "macos")]
fn basic_gpu_info() -> Vec<GpuMetric> {
    basic_gpu_metrics(parse_system_profiler_gpu_names(&command(
        "system_profiler",
        &["SPDisplaysDataType"],
    )))
}

#[cfg(target_os = "freebsd")]
fn basic_gpu_info() -> Vec<GpuMetric> {
    basic_gpu_metrics(parse_pciconf_gpu_names(&command("pciconf", &["-lv"])))
}

fn gpu_info() -> Vec<GpuMetric> {
    let detailed = nvidia_gpu_info();
    if detailed.is_empty() {
        basic_gpu_info()
    } else {
        detailed
    }
}

fn average_gpu_usage(gpus: &[GpuMetric]) -> f64 {
    let usages = gpus.iter().filter_map(|gpu| gpu.usage).collect::<Vec<_>>();
    if usages.is_empty() {
        0.0
    } else {
        usages.iter().sum::<f64>() / usages.len() as f64
    }
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn per_second(value: u64, elapsed_seconds: f64) -> f64 {
    value as f64 / elapsed_seconds.max(0.001)
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

#[cfg(target_os = "linux")]
struct Collector {
    previous_cpu: CpuSample,
    previous_io: IoSample,
    previous_at: Instant,
    network_interface: String,
    basic: BasicMetrics,
    basic_at: Instant,
    slow: SlowMetrics,
    slow_at: Instant,
    public_ip: PublicIpProbe,
}

#[cfg(target_os = "linux")]
impl Collector {
    fn new(config: &RuntimeConfig) -> Self {
        let mut collector = Self {
            previous_cpu: cpu_sample(),
            previous_io: io_sample(&config.network_interface),
            previous_at: Instant::now(),
            network_interface: config.network_interface.clone(),
            basic: BasicMetrics::default(),
            basic_at: Instant::now(),
            slow: SlowMetrics::default(),
            slow_at: Instant::now(),
            public_ip: PublicIpProbe::default(),
        };
        collector.refresh_basic();
        collector.refresh_slow();
        collector
    }

    fn refresh_basic(&mut self) {
        let gpus = gpu_info();
        self.basic = BasicMetrics {
            cpu_cores: thread::available_parallelism().map_or(1, |value| value.get() as i64),
            cpu_model: cpu_model(),
            os: os_name(),
            kernel: command("uname", &["-r"]),
            arch: env::consts::ARCH.to_string(),
            virtualization: command("systemd-detect-virt", &[]),
            gpu_usage: average_gpu_usage(&gpus),
            gpu_model: gpus
                .iter()
                .map(|gpu| gpu.model.as_str())
                .collect::<Vec<_>>()
                .join(" · "),
            gpus,
        };
        self.basic_at = Instant::now();
    }

    fn refresh_slow(&mut self) {
        let processes = fs::read_dir("/proc").map_or(0, |items| {
            items
                .filter_map(std::result::Result::ok)
                .filter(|item| {
                    item.file_name()
                        .to_string_lossy()
                        .chars()
                        .all(|ch| ch.is_ascii_digit())
                })
                .count() as i64
        });
        self.slow = SlowMetrics {
            disks: disk_usage(),
            processes,
            tcp_connections: file_line_count("/proc/net/tcp") + file_line_count("/proc/net/tcp6"),
            udp_connections: file_line_count("/proc/net/udp") + file_line_count("/proc/net/udp6"),
        };
        self.slow_at = Instant::now();
    }

    fn collect(
        &mut self,
        config: &RuntimeConfig,
        latency_results: Vec<LatencyResult>,
        timestamp: i64,
    ) -> Report {
        let sampled_at = Instant::now();
        let elapsed = sampled_at
            .saturating_duration_since(self.previous_at)
            .as_secs_f64()
            .max(0.001);
        let cpu_now = cpu_sample();
        let io_now = io_sample(&config.network_interface);
        let total = cpu_now.total.saturating_sub(self.previous_cpu.total);
        let idle = cpu_now.idle.saturating_sub(self.previous_cpu.idle);
        let cpu = if total == 0 {
            0.0
        } else {
            (total.saturating_sub(idle)) as f64 * 100.0 / total as f64
        };
        let interface_changed = self.network_interface != config.network_interface;
        let io_before = if interface_changed {
            io_now
        } else {
            self.previous_io
        };
        self.previous_cpu = cpu_now;
        self.previous_io = io_now;
        self.previous_at = sampled_at;
        self.network_interface.clone_from(&config.network_interface);

        if self.basic_at.elapsed() >= BASIC_INFO_REFRESH_INTERVAL {
            self.refresh_basic();
        }
        if self.slow_at.elapsed() >= SLOW_METRICS_REFRESH_INTERVAL {
            self.refresh_slow();
        }
        self.public_ip.refresh();

        let mem = text("/proc/meminfo");
        let mem_total = mem_value(&mem, "MemTotal:");
        let mem_used = mem_total.saturating_sub(mem_value(&mem, "MemAvailable:"));
        let swap_total = mem_value(&mem, "SwapTotal:");
        let swap_used = swap_total.saturating_sub(mem_value(&mem, "SwapFree:"));
        let loads = text("/proc/loadavg")
            .split_whitespace()
            .take(3)
            .filter_map(|value| value.parse::<f64>().ok())
            .collect::<Vec<_>>();
        let uptime = text("/proc/uptime")
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0) as i64;
        let processes = self.slow.processes;
        let disks = self.slow.disks.clone();
        let disk_used = disks.iter().map(|disk| disk.used).sum();
        let disk_total = disks.iter().map(|disk| disk.total).sum();
        let tcp_connections = self.slow.tcp_connections;
        let udp_connections = self.slow.udp_connections;
        let read_ops = io_now.read_ops.saturating_sub(io_before.read_ops);
        let write_ops = io_now.write_ops.saturating_sub(io_before.write_ops);
        let total_ops = read_ops.saturating_add(write_ops);
        let io_wait = io_now
            .read_millis
            .saturating_sub(io_before.read_millis)
            .saturating_add(io_now.write_millis.saturating_sub(io_before.write_millis));
        let elapsed_ms = elapsed * 1000.0;

        Report {
            timestamp,
            cpu,
            load1: loads.first().copied().unwrap_or(0.0),
            load5: loads.get(1).copied().unwrap_or(0.0),
            load15: loads.get(2).copied().unwrap_or(0.0),
            mem_used,
            mem_total,
            swap_used,
            swap_total,
            disk_used,
            disk_total,
            net_in: per_second(io_now.rx.saturating_sub(io_before.rx), elapsed),
            net_out: per_second(io_now.tx.saturating_sub(io_before.tx), elapsed),
            net_rx_total: io_now.rx.min(i64::MAX as u64) as i64,
            net_tx_total: io_now.tx.min(i64::MAX as u64) as i64,
            uptime,
            processes,
            tcp_connections,
            udp_connections,
            cpu_cores: self.basic.cpu_cores,
            cpu_model: self.basic.cpu_model.clone(),
            os: self.basic.os.clone(),
            kernel: self.basic.kernel.clone(),
            arch: self.basic.arch.clone(),
            virtualization: self.basic.virtualization.clone(),
            gpu_usage: self.basic.gpu_usage,
            gpu_model: self.basic.gpu_model.clone(),
            agent_version: VERSION.to_string(),
            ip_v4: self.public_ip.v4().unwrap_or_default(),
            ip_v6: self.public_ip.v6().unwrap_or_default(),
            disk_read_bps: per_second(
                io_now
                    .read_sectors
                    .saturating_sub(io_before.read_sectors)
                    .saturating_mul(512),
                elapsed,
            ),
            disk_write_bps: per_second(
                io_now
                    .write_sectors
                    .saturating_sub(io_before.write_sectors)
                    .saturating_mul(512),
                elapsed,
            ),
            disk_read_iops: per_second(read_ops, elapsed),
            disk_write_iops: per_second(write_ops, elapsed),
            disk_await_ms: if total_ops == 0 {
                0.0
            } else {
                io_wait as f64 / total_ops as f64
            },
            disk_utilization: if elapsed_ms <= 0.0 {
                0.0
            } else {
                (io_now.io_millis.saturating_sub(io_before.io_millis) as f64 * 100.0 / elapsed_ms)
                    .clamp(0.0, 100.0)
            },
            disks,
            gpus: self.basic.gpus.clone(),
            latency_results,
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
struct Collector {
    system: System,
    networks: Networks,
    disks: Disks,
    previous_at: Instant,
    basic: BasicMetrics,
    basic_at: Instant,
    slow: SlowMetrics,
    slow_at: Instant,
    public_ip: PublicIpProbe,
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
impl Collector {
    fn new(_config: &RuntimeConfig) -> Self {
        let mut collector = Self {
            system: System::new_all(),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            previous_at: Instant::now(),
            basic: BasicMetrics::default(),
            basic_at: Instant::now(),
            slow: SlowMetrics::default(),
            slow_at: Instant::now(),
            public_ip: PublicIpProbe::default(),
        };
        collector.refresh_basic();
        collector.refresh_slow();
        collector
    }

    fn refresh_basic(&mut self) {
        let gpus = gpu_info();
        self.basic = BasicMetrics {
            cpu_cores: self.system.cpus().len().max(1) as i64,
            cpu_model: self
                .system
                .cpus()
                .first()
                .map(|cpu| cpu.brand().to_string())
                .unwrap_or_default(),
            os: System::long_os_version().unwrap_or_else(|| env::consts::OS.to_string()),
            kernel: System::kernel_version().unwrap_or_default(),
            arch: env::consts::ARCH.to_string(),
            virtualization: String::new(),
            gpu_usage: average_gpu_usage(&gpus),
            gpu_model: gpus
                .iter()
                .map(|gpu| gpu.model.as_str())
                .collect::<Vec<_>>()
                .join(" · "),
            gpus,
        };
        self.basic_at = Instant::now();
    }

    fn refresh_slow(&mut self) {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
        self.disks.refresh(true);
        #[cfg(target_os = "windows")]
        let root = format!(
            "{}\\",
            env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string())
        );
        #[cfg(any(target_os = "macos", target_os = "freebsd"))]
        let root = "/".to_string();
        let mut disks = self
            .disks
            .iter()
            .filter(|disk| !disk.is_removable())
            .map(|disk| {
                let total = disk.total_space();
                DiskMetric {
                    name: disk.name().to_string_lossy().to_string(),
                    mount_point: disk.mount_point().to_string_lossy().to_string(),
                    used: u64_to_i64(total.saturating_sub(disk.available_space())),
                    total: u64_to_i64(total),
                    ..DiskMetric::default()
                }
            })
            .collect::<Vec<_>>();
        disks.sort_by_key(|disk| (disk.mount_point != root, disk.mount_point.clone()));
        let (tcp_connections, udp_connections) = connection_counts();
        self.slow = SlowMetrics {
            disks,
            processes: self.system.processes().len() as i64,
            tcp_connections,
            udp_connections,
        };
        self.slow_at = Instant::now();
    }

    fn collect(
        &mut self,
        config: &RuntimeConfig,
        latency_results: Vec<LatencyResult>,
        timestamp: i64,
    ) -> Report {
        let sampled_at = Instant::now();
        let elapsed = sampled_at
            .saturating_duration_since(self.previous_at)
            .as_secs_f64()
            .max(0.001);
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.networks.refresh(true);
        self.previous_at = sampled_at;
        if self.basic_at.elapsed() >= BASIC_INFO_REFRESH_INTERVAL {
            self.refresh_basic();
        }
        if self.slow_at.elapsed() >= SLOW_METRICS_REFRESH_INTERVAL {
            self.refresh_slow();
        }
        self.public_ip.refresh();

        let disk_metrics = self.slow.disks.clone();
        let tcp_connections = self.slow.tcp_connections;
        let udp_connections = self.slow.udp_connections;

        let mut net_in = 0_u64;
        let mut net_out = 0_u64;
        let mut net_rx_total = 0_u64;
        let mut net_tx_total = 0_u64;
        for (name, network) in &self.networks {
            if !selected_interface(name, &config.network_interface) {
                continue;
            }
            net_in = net_in.saturating_add(network.received());
            net_out = net_out.saturating_add(network.transmitted());
            net_rx_total = net_rx_total.saturating_add(network.total_received());
            net_tx_total = net_tx_total.saturating_add(network.total_transmitted());
        }
        let load = System::load_average();
        Report {
            timestamp,
            cpu: self.system.global_cpu_usage() as f64,
            load1: load.one,
            load5: load.five,
            load15: load.fifteen,
            mem_used: u64_to_i64(self.system.used_memory()),
            mem_total: u64_to_i64(self.system.total_memory()),
            swap_used: u64_to_i64(self.system.used_swap()),
            swap_total: u64_to_i64(self.system.total_swap()),
            disk_used: disk_metrics.iter().map(|disk| disk.used).sum(),
            disk_total: disk_metrics.iter().map(|disk| disk.total).sum(),
            net_in: per_second(net_in, elapsed),
            net_out: per_second(net_out, elapsed),
            net_rx_total: u64_to_i64(net_rx_total),
            net_tx_total: u64_to_i64(net_tx_total),
            uptime: u64_to_i64(System::uptime()),
            processes: self.slow.processes,
            tcp_connections,
            udp_connections,
            cpu_cores: self.basic.cpu_cores,
            cpu_model: self.basic.cpu_model.clone(),
            os: self.basic.os.clone(),
            kernel: self.basic.kernel.clone(),
            arch: self.basic.arch.clone(),
            virtualization: self.basic.virtualization.clone(),
            gpu_usage: self.basic.gpu_usage,
            gpu_model: self.basic.gpu_model.clone(),
            agent_version: VERSION.to_string(),
            ip_v4: self.public_ip.v4().unwrap_or_default(),
            ip_v6: self.public_ip.v6().unwrap_or_default(),
            disk_read_bps: 0.0,
            disk_write_bps: 0.0,
            disk_read_iops: 0.0,
            disk_write_iops: 0.0,
            disk_await_ms: 0.0,
            disk_utilization: 0.0,
            disks: disk_metrics,
            gpus: self.basic.gpus.clone(),
            latency_results,
        }
    }
}

fn runtime_config(options: &CliOptions) -> Result<RuntimeConfig> {
    let interval = options.interval;
    if !(15..=3600).contains(&interval) {
        return Err("interval must be between 15 and 3600 seconds".into());
    }
    let token = options
        .token
        .clone()
        .or_else(|| env::var("NODEFLARE_AGENT_TOKEN").ok())
        .unwrap_or_default();
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
        collect_interval: telemetry::MIN_COLLECT_INTERVAL,
        network_interface: String::new(),
        agent_mirror: String::new(),
        auto_update: true,
        latency_tasks: Vec::new(),
    })
}

fn live_endpoint(endpoint: &str) -> Result<String> {
    let mut url = url::Url::parse(endpoint)?;
    url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
        .map_err(|_| "unsupported live endpoint scheme")?;
    let path = format!("{}/api/agent/ws", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url.to_string())
}

type LiveSocket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

fn connect_live(endpoint: &str, token: &str) -> Result<(LiveSocket, Option<i64>)> {
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
    Ok((socket, clock_offset_ms))
}

fn connect_live_stream(url: &url::Url, deadline: Instant) -> Result<TcpStream> {
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

fn set_live_read_timeout(socket: &mut LiveSocket, timeout: Option<Duration>) -> io::Result<()> {
    match socket.get_mut() {
        tungstenite::stream::MaybeTlsStream::Plain(stream) => stream.set_read_timeout(timeout),
        tungstenite::stream::MaybeTlsStream::Rustls(stream) => {
            stream.sock.set_read_timeout(timeout)
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveAck {
    #[serde(rename = "type")]
    message_type: String,
    ts: i64,
    persisted: bool,
    #[serde(rename = "persistenceError")]
    persistence_error: bool,
    #[serde(rename = "persistedThroughTs")]
    persisted_through_ts: i64,
    #[serde(rename = "nextPersistAfterMs")]
    next_persist_after_ms: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LiveConfigMessage {
    #[serde(rename = "type")]
    message_type: String,
    ts: i64,
    config: RemoteConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteTaskMessage {
    #[serde(rename = "type")]
    message_type: String,
    task_id: String,
    command: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct TaskResultMessage {
    #[serde(rename = "type")]
    message_type: String,
    task_id: String,
    status: String,
    result: String,
    exit_code: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskResultAckMessage {
    #[serde(rename = "type")]
    message_type: String,
    task_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RemoteTaskJournal {
    version: u32,
    entries: Vec<RemoteTaskJournalEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum RemoteTaskJournalEntry {
    Active { task_id: String },
    Completed { result: TaskResultMessage },
}

#[derive(Default)]
struct CapturedOutput {
    bytes: Vec<u8>,
    truncated: bool,
}

fn capture_remote_output<R>(mut reader: R) -> mpsc::Receiver<CapturedOutput>
where
    R: Read + Send + 'static,
{
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut truncated = false;
        let mut buffer = [0_u8; 8192];
        // Keep draining after the limit so verbose commands do not receive SIGPIPE.
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    let retained = read.min(REMOTE_STREAM_OUTPUT_BYTES as usize - bytes.len());
                    bytes.extend_from_slice(&buffer[..retained]);
                    truncated |= retained < read;
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = sender.send(CapturedOutput { bytes, truncated });
    });
    receiver
}

fn terminate_remote_process_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let process_group = format!("-{}", child.id());
        let _ = Command::new("kill")
            .args(["-KILL", "--", process_group.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn receive_remote_output(
    receiver: &mpsc::Receiver<CapturedOutput>,
    child: &mut Child,
) -> CapturedOutput {
    match receiver.recv_timeout(REMOTE_OUTPUT_DRAIN_TIMEOUT) {
        Ok(output) => output,
        Err(_) => {
            // Descendants may keep the pipe open after the shell exits.
            terminate_remote_process_tree(child);
            receiver
                .recv_timeout(REMOTE_OUTPUT_DRAIN_TIMEOUT)
                .unwrap_or_default()
        }
    }
}

fn remote_result_text(stdout: &CapturedOutput, stderr: &CapturedOutput) -> String {
    let truncated = stdout.truncated || stderr.truncated;
    let stdout = String::from_utf8_lossy(&stdout.bytes);
    let stderr = String::from_utf8_lossy(&stderr.bytes);
    let mut combined = if stderr.is_empty() {
        stdout.trim().to_string()
    } else if stdout.is_empty() {
        stderr.trim().to_string()
    } else {
        format!("{}\n{}", stdout.trim(), stderr.trim())
    };
    let result_truncated = combined.len() > REMOTE_RESULT_OUTPUT_BYTES;
    if result_truncated {
        let mut end = REMOTE_RESULT_OUTPUT_BYTES;
        while !combined.is_char_boundary(end) {
            end -= 1;
        }
        combined.truncate(end);
    }
    if truncated || result_truncated {
        combined.push_str("\n[输出已截断]");
    }
    combined
}

fn wait_for_remote_child(
    child: &mut Child,
    timeout: Duration,
) -> io::Result<Option<std::process::ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        thread::sleep(remaining.min(Duration::from_millis(50)));
    }
}

fn execute_remote_task_with_timeout(
    task: &RemoteTaskMessage,
    timeout: Duration,
) -> TaskResultMessage {
    let task_id = task.task_id.clone();

    if task.command.trim().is_empty() || task.command.len() > 16_384 {
        return TaskResultMessage {
            message_type: "task_result".to_string(),
            task_id,
            status: "failed".to_string(),
            result: "命令为空或长度超出限制".to_string(),
            exit_code: Some(-1),
        };
    }

    #[cfg(unix)]
    let mut command = {
        let mut command = Command::new("sh");
        command.args(["-c", task.command.as_str()]);
        command.process_group(0);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", task.command.as_str()]);
        command
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return TaskResultMessage {
                message_type: "task_result".to_string(),
                task_id,
                status: "failed".to_string(),
                result: format!("无法启动命令：{error}"),
                exit_code: Some(-1),
            };
        }
    };
    let stdout = capture_remote_output(child.stdout.take().expect("stdout is piped"));
    let stderr = capture_remote_output(child.stderr.take().expect("stderr is piped"));

    let status = match wait_for_remote_child(&mut child, timeout) {
        Ok(Some(status)) => Some(status),
        Ok(None) => {
            terminate_remote_process_tree(&mut child);
            None
        }
        Err(error) => {
            terminate_remote_process_tree(&mut child);
            let stdout = receive_remote_output(&stdout, &mut child);
            let stderr = receive_remote_output(&stderr, &mut child);
            let output = remote_result_text(&stdout, &stderr);
            return TaskResultMessage {
                message_type: "task_result".to_string(),
                task_id,
                status: "failed".to_string(),
                result: if output.is_empty() {
                    format!("等待命令结束失败：{error}")
                } else {
                    format!("{output}\n等待命令结束失败：{error}")
                },
                exit_code: Some(-1),
            };
        }
    };
    let stdout = receive_remote_output(&stdout, &mut child);
    let stderr = receive_remote_output(&stderr, &mut child);
    let output = remote_result_text(&stdout, &stderr);

    let Some(status) = status else {
        let timeout_seconds = timeout.as_secs().max(1);
        return TaskResultMessage {
            message_type: "task_result".to_string(),
            task_id,
            status: "failed".to_string(),
            result: if output.is_empty() {
                format!("命令执行超过 {timeout_seconds} 秒，已终止")
            } else {
                format!("{output}\n命令执行超过 {timeout_seconds} 秒，已终止")
            },
            exit_code: Some(-1),
        };
    };

    TaskResultMessage {
        message_type: "task_result".to_string(),
        task_id,
        status: if status.success() {
            "success"
        } else {
            "failed"
        }
        .to_string(),
        result: output,
        exit_code: status.code().map(i64::from),
    }
}

fn execute_remote_task(task: &RemoteTaskMessage) -> TaskResultMessage {
    execute_remote_task_with_timeout(task, REMOTE_TASK_TIMEOUT)
}

struct RemoteExecutor {
    task_tx: mpsc::SyncSender<RemoteTaskMessage>,
    result_rx: mpsc::Receiver<TaskResultMessage>,
    active: HashSet<String>,
    completed: HashMap<String, TaskResultMessage>,
    task_order: VecDeque<String>,
    last_sent: HashMap<String, Instant>,
    journal_path: PathBuf,
    disabled: bool,
}

impl RemoteExecutor {
    fn new() -> Result<Self> {
        Self::new_at(remote_task_journal_path()?)
    }

    fn new_at(journal_path: PathBuf) -> Result<Self> {
        let journal = load_remote_task_journal(&journal_path)?;
        let mut active = HashSet::new();
        let mut completed = HashMap::new();
        let mut task_order = VecDeque::new();
        for entry in journal.entries {
            match entry {
                RemoteTaskJournalEntry::Active { task_id } => {
                    active.insert(task_id.clone());
                    task_order.push_back(task_id);
                }
                RemoteTaskJournalEntry::Completed { result } => {
                    task_order.push_back(result.task_id.clone());
                    completed.insert(result.task_id.clone(), result);
                }
            }
        }

        let (task_tx, task_rx) = mpsc::sync_channel::<RemoteTaskMessage>(16);
        let (result_tx, result_rx) = mpsc::channel::<TaskResultMessage>();
        thread::Builder::new()
            .name("nodeflare-remote".to_string())
            .spawn(move || {
                while let Ok(task) = task_rx.recv() {
                    if result_tx.send(execute_remote_task(&task)).is_err() {
                        return;
                    }
                }
            })?;
        let interrupted = active.drain().collect::<Vec<_>>();
        for task_id in interrupted {
            completed.insert(
                task_id.clone(),
                TaskResultMessage {
                    message_type: "task_result".to_string(),
                    task_id,
                    status: "failed".to_string(),
                    result: "Agent 在命令执行期间重启，无法确认原命令状态；为避免重复操作，任务不会再次执行"
                        .to_string(),
                    exit_code: Some(-1),
                },
            );
        }
        let executor = Self {
            task_tx,
            result_rx,
            active,
            completed,
            task_order,
            last_sent: HashMap::new(),
            journal_path,
            disabled: false,
        };
        if !executor.completed.is_empty() {
            executor.persist_snapshot(None)?;
        }
        Ok(executor)
    }

    fn disabled() -> Self {
        let (task_tx, task_rx) = mpsc::sync_channel(1);
        let (result_tx, result_rx) = mpsc::channel();
        drop(task_rx);
        drop(result_tx);
        Self {
            task_tx,
            result_rx,
            active: HashSet::new(),
            completed: HashMap::new(),
            task_order: VecDeque::new(),
            last_sent: HashMap::new(),
            journal_path: PathBuf::new(),
            disabled: true,
        }
    }

    fn enqueue(&mut self, task: RemoteTaskMessage) -> Option<TaskResultMessage> {
        if self.disabled {
            return Some(remote_task_rejected(
                task.task_id,
                "Agent 任务日志异常，远程执行已禁用",
            ));
        }
        if self.completed.contains_key(&task.task_id) {
            self.last_sent.remove(&task.task_id);
            return None;
        }
        if self.active.contains(&task.task_id) {
            return None;
        }
        let task_id = task.task_id.clone();
        if self.task_order.len() >= MAX_REMOTE_TASK_JOURNAL_ENTRIES {
            return Some(remote_task_rejected(
                task_id,
                "待确认的远程任务过多，已拒绝执行",
            ));
        }
        self.active.insert(task_id.clone());
        self.task_order.push_back(task_id.clone());
        if let Err(error) = self.persist_snapshot(None) {
            self.active.remove(&task_id);
            self.task_order.retain(|id| id != &task_id);
            eprintln!("remote task journal write failed: {error}");
            return Some(remote_task_rejected(
                task_id,
                "Agent 无法持久化任务状态，已拒绝执行",
            ));
        }
        match self.task_tx.try_send(task) {
            Ok(()) => None,
            Err(error) => {
                let message = match error {
                    mpsc::TrySendError::Full(_) => "远程命令队列已满，请稍后重试",
                    mpsc::TrySendError::Disconnected(_) => "远程命令执行线程不可用",
                };
                let result = remote_task_rejected(task_id, message);
                self.remember_completed(result.clone());
                None
            }
        }
    }

    fn drain_completed(&mut self) {
        let results = self.result_rx.try_iter().collect::<Vec<_>>();
        for result in results {
            self.remember_completed(result);
        }
    }

    fn acknowledge(&mut self, task_id: &str) {
        if !self.completed.contains_key(task_id) {
            return;
        }
        match self.persist_snapshot(Some(task_id)) {
            Ok(()) => {
                self.completed.remove(task_id);
                self.task_order.retain(|id| id != task_id);
                self.last_sent.remove(task_id);
            }
            Err(error) => {
                eprintln!("remote task ACK persistence failed: {error}");
            }
        }
    }

    fn remember_completed(&mut self, result: TaskResultMessage) {
        let task_id = result.task_id.clone();
        let was_active = self.active.remove(&task_id);
        if !was_active && !self.completed.contains_key(&task_id) {
            self.task_order.push_back(task_id.clone());
        }
        self.completed.insert(task_id.clone(), result);
        self.last_sent.remove(&task_id);
        if let Err(error) = self.persist_snapshot(None) {
            eprintln!("remote task result persistence failed: {error}");
        }
    }

    fn due_results(&mut self) -> Vec<TaskResultMessage> {
        self.drain_completed();
        let now = Instant::now();
        self.task_order
            .iter()
            .filter_map(|task_id| {
                let result = self.completed.get(task_id)?;
                let due = self.last_sent.get(task_id).is_none_or(|sent_at| {
                    now.duration_since(*sent_at) >= REMOTE_RESULT_RETRY_INTERVAL
                });
                due.then(|| result.clone())
            })
            .collect()
    }

    fn mark_sent(&mut self, task_id: &str) {
        self.last_sent.insert(task_id.to_string(), Instant::now());
    }

    fn reset_delivery(&mut self) {
        self.last_sent.clear();
    }

    fn persist_snapshot(&self, excluded_task_id: Option<&str>) -> Result<()> {
        let entries = self
            .task_order
            .iter()
            .filter(|task_id| excluded_task_id != Some(task_id.as_str()))
            .filter_map(|task_id| {
                if self.active.contains(task_id) {
                    Some(RemoteTaskJournalEntry::Active {
                        task_id: task_id.clone(),
                    })
                } else {
                    self.completed
                        .get(task_id)
                        .cloned()
                        .map(|result| RemoteTaskJournalEntry::Completed { result })
                }
            })
            .collect::<Vec<_>>();
        write_remote_task_journal(&self.journal_path, entries)
    }
}

fn remote_task_rejected(task_id: String, result: &str) -> TaskResultMessage {
    TaskResultMessage {
        message_type: "task_result".to_string(),
        task_id,
        status: "failed".to_string(),
        result: result.to_string(),
        exit_code: Some(-1),
    }
}

fn valid_remote_task_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn load_remote_task_journal(path: &Path) -> Result<RemoteTaskJournal> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(RemoteTaskJournal {
                version: REMOTE_TASK_JOURNAL_VERSION,
                entries: Vec::new(),
            });
        }
        Err(error) => return Err(error.into()),
    };
    if bytes.len() as u64 > MAX_REMOTE_TASK_JOURNAL_BYTES {
        return Err("remote task journal is too large".into());
    }
    let journal = serde_json::from_slice::<RemoteTaskJournal>(&bytes)?;
    if journal.version != REMOTE_TASK_JOURNAL_VERSION {
        return Err("unsupported remote task journal version".into());
    }
    if journal.entries.len() > MAX_REMOTE_TASK_JOURNAL_ENTRIES {
        return Err("remote task journal has too many entries".into());
    }
    let mut task_ids = HashSet::new();
    for entry in &journal.entries {
        let (task_id, valid_result) = match entry {
            RemoteTaskJournalEntry::Active { task_id } => (task_id, true),
            RemoteTaskJournalEntry::Completed { result } => (
                &result.task_id,
                result.message_type == "task_result"
                    && matches!(result.status.as_str(), "success" | "failed")
                    && result.result.len() <= REMOTE_RESULT_OUTPUT_BYTES + 64,
            ),
        };
        if !valid_remote_task_id(task_id) || !valid_result || !task_ids.insert(task_id) {
            return Err("remote task journal contains an invalid entry".into());
        }
    }
    Ok(journal)
}

fn write_remote_task_journal(path: &Path, entries: Vec<RemoteTaskJournalEntry>) -> Result<()> {
    if entries.is_empty() {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
    }
    let journal = RemoteTaskJournal {
        version: REMOTE_TASK_JOURNAL_VERSION,
        entries,
    };
    let mut encoded = serde_json::to_vec(&journal)?;
    encoded.push(b'\n');
    if encoded.len() as u64 > MAX_REMOTE_TASK_JOURNAL_BYTES {
        return Err("remote task journal is too large".into());
    }
    let temporary = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("remote-tasks"),
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    drop(file);
    #[cfg(target_os = "windows")]
    if path.exists() {
        fs::remove_file(path)?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn flush_remote_results(socket: &mut LiveSocket, executor: &mut RemoteExecutor) -> Result<()> {
    for result in executor.due_results() {
        socket.send(Message::Text(serde_json::to_string(&result)?.into()))?;
        executor.mark_sent(&result.task_id);
    }
    Ok(())
}

fn ack_persist_interval(ack: &LiveAck) -> Duration {
    Duration::from_millis(ack.next_persist_after_ms)
        .clamp(Duration::from_secs(1), Duration::from_secs(3600))
}

enum LiveRead {
    Ack(LiveAck),
    Config(RemoteConfig),
    RemoteTask(RemoteTaskMessage),
    TaskResultAck(String),
    Closed,
    Pending,
}

fn read_live_ack(socket: &mut LiveSocket) -> Result<LiveRead> {
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

fn accept_remote_task(
    socket: &mut LiveSocket,
    executor: &mut RemoteExecutor,
    task: RemoteTaskMessage,
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

fn wait_for_live_ack(
    socket: &mut LiveSocket,
    remote_config: &Arc<Mutex<Option<RemoteConfig>>>,
    remote_executor: &mut RemoteExecutor,
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

fn live_update_payload(
    reports: Vec<Report>,
    persist: bool,
    info: &mut Option<telemetry::Info>,
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

fn observe_persisted_through(target: &AtomicI64, ack: &LiveAck) {
    if ack.persisted && !ack.persistence_error {
        target.fetch_max(ack.persisted_through_ts.max(0), Ordering::AcqRel);
    }
}

fn prune_live_queue(pending: &Arc<(Mutex<VecDeque<Report>>, Condvar)>, persisted_through: i64) {
    if persisted_through <= 0 {
        return;
    }
    if let Ok(mut queue) = pending.0.lock() {
        queue.retain(|report| report.timestamp > persisted_through);
    }
}

fn live_batch_after(queue: &VecDeque<Report>, timestamp: i64) -> Vec<Report> {
    let candidates = queue
        .iter()
        .filter(|report| report.timestamp > timestamp)
        .cloned()
        .collect::<VecDeque<_>>();
    let count = live_batch::batch_len(&candidates);
    candidates.into_iter().take(count).collect()
}

fn live_sender_loop(endpoint: &str, token: &str, worker: LiveSenderWorker) {
    let LiveSenderWorker {
        pending,
        configured_interval,
        persisted_through,
        remote_config,
        clock,
        stats,
    } = worker;
    let mut socket: Option<LiveSocket> = None;
    let mut wss_interval = configured_interval.lock().map_or(
        Duration::from_secs(telemetry::MIN_COLLECT_INTERVAL),
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
                        thread::sleep(LIVE_RECONNECT_DELAY);
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
                    thread::sleep(LIVE_RECONNECT_DELAY);
                    continue;
                }
            }
        }

        if let Some(connected) = socket.as_mut()
            && flush_remote_results(connected, &mut remote_executor).is_err()
        {
            socket = None;
            thread::sleep(LIVE_RECONNECT_DELAY);
            continue;
        }

        let (queue_lock, ready) = &*pending;
        let Ok(mut queue) = queue_lock.lock() else {
            return;
        };
        let (batch, persist, more_pending) = loop {
            let now = Instant::now();
            let persisted = persisted_through.load(Ordering::Acquire);
            let unsent = live_batch_after(&queue, accepted_through);
            let persist =
                now >= next_probe_at && queue.iter().any(|report| report.timestamp > persisted);
            if persist || (!unsent.is_empty() && now >= next_send_at) {
                let last = unsent
                    .last()
                    .map_or(accepted_through, |report| report.timestamp);
                let more_pending = queue.iter().any(|report| report.timestamp > last);
                // A commit can be empty: samples already sent remain buffered by the server.
                break (unsent, persist || more_pending, more_pending);
            }

            let wake_at = if !unsent.is_empty() {
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
                thread::sleep(LIVE_RECONNECT_DELAY);
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
                Duration::from_secs(telemetry::MIN_COLLECT_INTERVAL),
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
            thread::sleep(LIVE_RECONNECT_DELAY);
            continue;
        }
        stats.live_batch_sent(batch_len);
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
            thread::sleep(LIVE_RECONNECT_DELAY);
        }
    }
}

fn apply_remote(config: &mut RuntimeConfig, remote: &RemoteConfig) -> bool {
    if valid_sample_schedule(remote.report_interval, remote.collect_interval) {
        config.report_interval = remote.report_interval;
        config.collect_interval = remote.collect_interval.max(telemetry::MIN_COLLECT_INTERVAL);
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
    #[cfg(target_os = "windows")]
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

fn normalized_version(value: &str) -> &str {
    let value = value.trim();
    value.strip_prefix('v').unwrap_or(value)
}

fn version_triplet(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = normalized_version(value).split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

fn agent_artifact_name() -> Option<&'static str> {
    match (env::consts::OS, env::consts::ARCH) {
        ("linux", "x86_64") => Some(if cfg!(target_env = "musl") {
            "agent-linux-x64-musl"
        } else {
            "agent-linux-x64-glibc"
        }),
        ("linux", "aarch64") => Some(if cfg!(target_env = "musl") {
            "agent-linux-aarch64-musl"
        } else {
            "agent-linux-aarch64-glibc"
        }),
        ("windows", "x86_64") => Some("agent-windows-x64.exe"),
        ("macos", "aarch64") => Some("agent-macos-aarch64"),
        ("freebsd", "x86_64") => Some("agent-freebsd-x64"),
        ("freebsd", "aarch64") => Some("agent-freebsd-aarch64"),
        _ => None,
    }
}

fn executable_format_valid(path: &Path) -> bool {
    let mut magic = [0_u8; 4];
    if fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_err()
    {
        return false;
    }
    match env::consts::OS {
        "linux" | "freebsd" => magic == *b"\x7fELF",
        "windows" => magic.starts_with(b"MZ"),
        "macos" => matches!(
            magic,
            [0xcf, 0xfa, 0xed, 0xfe]
                | [0xfe, 0xed, 0xfa, 0xcf]
                | [0xca, 0xfe, 0xba, 0xbe]
                | [0xbe, 0xba, 0xfe, 0xca]
        ),
        _ => false,
    }
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn release_asset_sha256(asset: &GithubReleaseAsset) -> Option<String> {
    let digest = asset.digest.as_deref()?.strip_prefix("sha256:")?;
    (digest.len() == 64
        && digest
            .chars()
            .all(|character| character.is_ascii_hexdigit()))
    .then(|| digest.to_ascii_lowercase())
}

fn download_agent(
    agent: &ureq::Agent,
    url: &str,
    destination: &Path,
    expected_version: &str,
    expected_sha256: &str,
) -> Result<bool> {
    let Ok(response) = agent.get(url).call() else {
        return Ok(false);
    };
    let response = response.into_body().into_reader();
    let mut file = fs::File::create(destination)?;
    let copied = io::copy(&mut response.take(MAX_AGENT_BINARY_BYTES + 1), &mut file)?;
    file.flush()?;
    drop(file);
    if copied > MAX_AGENT_BINARY_BYTES
        || !executable_format_valid(destination)
        || sha256_file(destination)? != expected_sha256
    {
        let _ = fs::remove_file(destination);
        return Ok(false);
    }
    #[cfg(unix)]
    fs::set_permissions(destination, fs::Permissions::from_mode(0o755))?;

    let downloaded_version = Command::new(destination)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .split_whitespace()
                .next_back()
                .map(str::to_string)
        });
    let valid = downloaded_version.as_deref().map(normalized_version)
        == Some(normalized_version(expected_version));
    if !valid {
        let _ = fs::remove_file(destination);
    }
    Ok(valid)
}

fn mirrored_download_url(mirror: &str, url: &str) -> String {
    let mirror = mirror.trim().trim_end_matches('/');
    if mirror.is_empty() {
        url.to_string()
    } else {
        format!("{mirror}/{url}")
    }
}

fn update(agent: &ureq::Agent, mirror: &str) -> Result<bool> {
    let mut response = agent
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", format!("nodeflare-agent/{VERSION}"))
        .call()?;
    let release = response.body_mut().read_json::<GithubRelease>()?;
    let Some(remote_version) = version_triplet(&release.tag_name) else {
        return Ok(false);
    };
    let Some(current_version) = version_triplet(VERSION) else {
        return Ok(false);
    };
    if remote_version <= current_version {
        return Ok(false);
    }
    let Some(artifact) = agent_artifact_name() else {
        return Ok(false);
    };
    let current = env::current_exe()?;
    let temporary = current.with_file_name(format!(
        ".nodeflare-agent.{}.download{}",
        std::process::id(),
        if cfg!(target_os = "windows") {
            ".exe"
        } else {
            ""
        }
    ));
    let Some(release_asset) = release.assets.iter().find(|asset| asset.name == artifact) else {
        return Err(format!("latest release does not contain {artifact}").into());
    };
    let Some(expected_sha256) = release_asset_sha256(release_asset) else {
        return Err(
            format!("latest release does not contain a SHA-256 digest for {artifact}").into(),
        );
    };
    if !download_agent(
        agent,
        &mirrored_download_url(mirror, &release_asset.browser_download_url),
        &temporary,
        &release.tag_name,
        &expected_sha256,
    )? {
        return Err("downloaded agent version does not match the configured version".into());
    }

    #[cfg(unix)]
    {
        fs::rename(&temporary, &current)?;
        let error = Command::new(&current).args(env::args_os().skip(1)).exec();
        Err(error.into())
    }

    #[cfg(target_os = "windows")]
    {
        let script = concat!(
            "$ErrorActionPreference='Stop'; ",
            "$targetPid=[int]$env:NODEFLARE_UPDATE_PID; ",
            "Wait-Process -Id $targetPid; ",
            "Move-Item -LiteralPath $env:NODEFLARE_UPDATE_NEW ",
            "-Destination $env:NODEFLARE_UPDATE_CURRENT -Force; ",
            "try { Start-ScheduledTask -TaskName 'nodeflare-agent' -ErrorAction Stop } ",
            "catch { $restartArgs=@($env:NODEFLARE_UPDATE_ARGS | ConvertFrom-Json); ",
            "Start-Process -FilePath $env:NODEFLARE_UPDATE_CURRENT -ArgumentList $restartArgs }"
        );
        Command::new("powershell.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ])
            .env("NODEFLARE_UPDATE_PID", std::process::id().to_string())
            .env("NODEFLARE_UPDATE_NEW", &temporary)
            .env("NODEFLARE_UPDATE_CURRENT", &current)
            .env(
                "NODEFLARE_UPDATE_ARGS",
                serde_json::to_string(&env::args().skip(1).collect::<Vec<_>>())?,
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(true)
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
    let mut next_collect = Instant::now() + Duration::from_secs(config.collect_interval);
    let mut next_latency: HashMap<String, Instant> = HashMap::new();
    let stats = Arc::new(runtime_stats::RuntimeStats::default());
    let mut latency_executor = LatencyExecutor::new(Arc::clone(&stats))?;
    let mut pending_results = Vec::new();
    let spool_path = pending_spool_path()?;
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
            if before != pending_samples.len()
                && let Err(error) = rewrite_pending_spool(&spool_path, &pending_samples)
            {
                eprintln!("pending report spool compaction failed: {error}");
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
                if config.collect_interval != previous_collect_interval {
                    next_collect = Instant::now() + Duration::from_secs(config.collect_interval);
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

        // Check after ACK pruning, before the next sample refills the pending queue.
        if !once
            && !print_only
            && config.auto_update
            && pending_samples.is_empty()
            && Instant::now() >= next_update_check
        {
            match update(&agent, &config.agent_mirror) {
                Ok(true) => return Ok(()),
                Ok(false) => next_update_check = Instant::now() + UPDATE_CHECK_INTERVAL,
                Err(error) => {
                    eprintln!("agent update failed: {error}");
                    next_update_check = Instant::now() + UPDATE_RETRY_INTERVAL;
                }
            }
        }
        pending_results.extend(latency_executor.drain());
        let current = Instant::now();
        let due = config
            .latency_tasks
            .iter()
            .filter(|task| {
                next_latency
                    .get(&task.id)
                    .is_none_or(|deadline| *deadline <= current)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !due.is_empty() {
            let scheduled_at = Instant::now();
            for task in due {
                let task_id = task.id.clone();
                let interval = task.interval_seconds.clamp(30, 3600);
                if latency_executor.enqueue(task) {
                    next_latency.insert(task_id, scheduled_at + Duration::from_secs(interval));
                }
            }
        }

        if Instant::now() >= next_collect && (!once || once_target_timestamp.is_none()) {
            let offset_ms = shared_clock_offset(&clock);
            let mut latest_results = HashMap::new();
            for mut result in std::mem::take(&mut pending_results) {
                result.timestamp = corrected_timestamp(result.timestamp, offset_ms);
                latest_results.insert(result.task_id.clone(), result);
            }
            let collection_started = Instant::now();
            let mut report = collector.collect(&config, latest_results.into_values().collect(), 0);
            stats.collection_finished(collection_started.elapsed());
            let persisted_through = live.as_ref().map_or(0, LiveSender::persisted_through);
            report.timestamp = monotonic_report_timestamp(
                corrected_timestamp(unix_timestamp(), offset_ms),
                last_emitted_timestamp,
                persisted_through,
            );
            last_emitted_timestamp = report.timestamp;
            if print_only {
                println!("{}", serde_json::to_string_pretty(&report)?);
                return Ok(());
            }
            append_pending_spool(&spool_path, &report)
                .map_err(|error| format!("pending report spool append failed: {error}"))?;
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
            if (before_prune != pending_samples.len()
                || latency_before_prune
                    != pending_samples
                        .iter()
                        .map(|report| report.latency_results.len())
                        .sum::<usize>())
                && let Err(error) = rewrite_pending_spool(&spool_path, &pending_samples)
            {
                eprintln!("pending report spool compaction failed: {error}");
            }
            next_collect = advance_deadline(
                next_collect,
                Duration::from_secs(config.collect_interval),
                Instant::now(),
            );
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
        let maintenance_wake_at = if config.auto_update && pending_samples.is_empty() {
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
mod tests {
    use clap::Parser;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::{
        CLOCK_CALIBRATION_MAX_AGE, CapturedOutput, CliOptions, ClockCalibration,
        GithubReleaseAsset, LatencyResult, LatencyTask, LiveAck, MAX_PENDING_LATENCY_RESULTS,
        PUBLIC_IP_STALE_AFTER, PublicIpValue, REMOTE_RESULT_OUTPUT_BYTES,
        REMOTE_STREAM_OUTPUT_BYTES, RemoteExecutor, RemoteTaskJournalEntry, RemoteTaskMessage,
        Report, TaskResultMessage, UPDATE_CHECK_JITTER_MAX_SECONDS, ack_persist_interval,
        advance_deadline, clock_offset_from_http_date, corrected_timestamp,
        execute_remote_task_with_timeout, gpu_name_from_uevent, is_public_probe_ip, live_endpoint,
        live_update_payload, monotonic_report_timestamp, normalized_version, parse_lspci_gpu_names,
        parse_pciconf_gpu_names, parse_probe_target, parse_public_ip,
        parse_system_profiler_gpu_names, ping_latencies, ping_latency, prune_report_samples,
        release_asset_sha256, remote_result_text, sanitize_latency_tasks, update_check_jitter,
        valid_endpoint, version_triplet, write_remote_task_journal,
    };

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
    use super::connection_counts_from_netstat;
    #[cfg(target_os = "linux")]
    use super::disk_device;

    #[test]
    fn parses_family_matched_public_ips() {
        let v4: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        let v6: std::net::IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(parse_public_ip("203.0.113.7\n", false), Some(v4));
        assert_eq!(parse_public_ip("2001:db8::1", true), Some(v6));
        assert_eq!(parse_public_ip("2001:db8::1", false), None);
        assert_eq!(parse_public_ip("203.0.113.7", true), None);
        assert_eq!(parse_public_ip("not an ip", false), None);
        assert_eq!(parse_public_ip("", false), None);
    }

    #[test]
    fn expires_public_ip_after_sustained_probe_failures() {
        let started_at = Instant::now();
        let address = "203.0.113.7".parse().unwrap();
        let mut value = PublicIpValue::default();

        value.observe(Some(address), started_at);
        value.observe(
            None,
            started_at + PUBLIC_IP_STALE_AFTER - Duration::from_secs(1),
        );
        assert_eq!(value.address, Some(address));

        value.observe(None, started_at + PUBLIC_IP_STALE_AFTER);
        assert_eq!(value.address, None);
    }

    #[test]
    fn parses_platform_gpu_names() {
        let lspci = "00:02.0 VGA compatible controller: Intel Corporation CometLake-S GT2 [UHD Graphics 630] (rev 05)\n\
                     01:00.0 Audio device: NVIDIA Corporation HDMI Audio\n\
                     02:00.0 3D controller: NVIDIA Corporation GA102 [GeForce RTX 3090] (rev a1)";
        assert_eq!(
            parse_lspci_gpu_names(lspci),
            [
                "Intel Corporation CometLake-S GT2 [UHD Graphics 630] (rev 05)",
                "NVIDIA Corporation GA102 [GeForce RTX 3090] (rev a1)",
            ]
        );

        let profiler = "Graphics/Displays:\n\n    Apple M3 Max:\n\n      Chipset Model: Apple M3 Max\n      Metal Support: Metal 3";
        assert_eq!(parse_system_profiler_gpu_names(profiler), ["Apple M3 Max"]);

        let pciconf = "vgapci0@pci0:0:2:0:\tclass=0x030000 rev=0x02 hdr=0x00 vendor=0x8086\n\
                       \tvendor     = 'Intel Corporation'\n\
                       \tdevice     = 'UHD Graphics 630'\n\
                       \tclass      = display\n\
                       em0@pci0:0:25:0:\tclass=0x020000 rev=0x05 hdr=0x00 vendor=0x8086\n\
                       \tvendor     = 'Intel Corporation'\n\
                       \tdevice     = 'Ethernet Connection'\n\
                       \tclass      = network";
        assert_eq!(
            parse_pciconf_gpu_names(pciconf),
            ["Intel Corporation UHD Graphics 630"]
        );

        assert_eq!(
            gpu_name_from_uevent("DRIVER=i915\nPCI_CLASS=30000\nPCI_ID=8086:591B"),
            Some("Intel Integrated Graphics".to_string())
        );
        assert_eq!(
            gpu_name_from_uevent("DRIVER=virtio_gpu\nPCI_CLASS=30000"),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn selects_whole_disk_devices() {
        assert!(disk_device("sda"));
        assert!(disk_device("nvme0n1"));
        assert!(!disk_device("sda1"));
        assert!(!disk_device("nvme0n1p1"));
    }

    #[test]
    fn parses_release_versions() {
        assert_eq!(normalized_version("v1.2.3"), "1.2.3");
        assert_eq!(version_triplet("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(version_triplet("1.2"), None);
        assert_eq!(version_triplet("1.2.3.4"), None);
        assert_eq!(version_triplet("rust-3"), None);
    }

    #[test]
    fn validates_release_asset_digests() {
        let hash = "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";
        let mut asset = GithubReleaseAsset {
            name: "agent-linux-x64-glibc".to_string(),
            browser_download_url: "https://example.com/agent".to_string(),
            digest: Some(format!("sha256:{hash}")),
        };
        let expected = hash.to_ascii_lowercase();
        assert_eq!(
            release_asset_sha256(&asset).as_deref(),
            Some(expected.as_str())
        );
        asset.digest = Some("sha512:0123".to_string());
        assert_eq!(release_asset_sha256(&asset), None);
        asset.digest = None;
        assert_eq!(release_asset_sha256(&asset), None);
    }

    #[test]
    fn validates_endpoints() {
        assert!(valid_endpoint("https://monitor.example.com/"));
        assert!(valid_endpoint("http://127.0.0.1:8787"));
        assert!(valid_endpoint("http://localhost:8787"));
        assert!(valid_endpoint("http://[::1]:8787"));
        assert!(!valid_endpoint("http://monitor.example.com"));
        assert!(!valid_endpoint("http://127.0.0.2:8787"));
        assert!(!valid_endpoint("monitor.example.com"));
        assert!(!valid_endpoint("https://"));
        assert!(!valid_endpoint("https://user@example.com"));
        assert!(!valid_endpoint("https://monitor.example.com/?token=abc"));
        assert!(!valid_endpoint("https://bad host.example"));
    }

    #[test]
    fn builds_live_websocket_endpoints() {
        assert_eq!(
            live_endpoint("https://monitor.example.com").unwrap(),
            "wss://monitor.example.com/api/agent/ws"
        );
        assert_eq!(
            live_endpoint("https://monitor.example.com/base/").unwrap(),
            "wss://monitor.example.com/base/api/agent/ws"
        );
        assert_eq!(
            live_endpoint("http://127.0.0.1:8787").unwrap(),
            "ws://127.0.0.1:8787/api/agent/ws"
        );
    }

    #[test]
    fn accepts_durable_ack_interval_and_limits_upload_frequency() {
        let ack: LiveAck = serde_json::from_str(
            r#"{"type":"ack","ts":100,"persisted":true,"persistenceError":false,"persistedThroughTs":100,"nextPersistAfterMs":60000}"#,
        )
        .unwrap();
        assert_eq!(ack_persist_interval(&ack), Duration::from_secs(60));
        let idle: LiveAck = serde_json::from_str(
            r#"{"type":"ack","ts":100,"persisted":true,"persistenceError":false,"persistedThroughTs":100,"nextPersistAfterMs":120000}"#,
        )
        .unwrap();
        assert_eq!(ack_persist_interval(&idle), Duration::from_secs(120));
        assert_eq!(super::live_batch_interval(1), Duration::from_secs(3));
        assert_eq!(super::live_batch_interval(3), Duration::from_secs(3));
        assert_eq!(super::live_batch_interval(15), Duration::from_secs(15));
    }

    #[cfg(unix)]
    #[test]
    fn remote_commands_capture_output_and_exit_status() {
        let result = execute_remote_task_with_timeout(
            &RemoteTaskMessage {
                message_type: "remote_task".to_string(),
                task_id: "task-output".to_string(),
                command: "printf stdout; printf stderr >&2; exit 7".to_string(),
            },
            Duration::from_secs(2),
        );
        assert_eq!(result.status, "failed");
        assert_eq!(result.exit_code, Some(7));
        assert_eq!(result.result, "stdout\nstderr");
    }

    #[cfg(unix)]
    #[test]
    fn remote_commands_are_terminated_after_the_deadline() {
        let result = execute_remote_task_with_timeout(
            &RemoteTaskMessage {
                message_type: "remote_task".to_string(),
                task_id: "task-timeout".to_string(),
                command: "sleep 5".to_string(),
            },
            Duration::from_millis(50),
        );
        assert_eq!(result.status, "failed");
        assert_eq!(result.exit_code, Some(-1));
        assert!(result.result.contains("已终止"));
    }

    #[test]
    fn combined_remote_output_is_bounded_at_utf8_boundaries() {
        let result = remote_result_text(
            &CapturedOutput {
                bytes: vec![b'x'; REMOTE_STREAM_OUTPUT_BYTES as usize],
                truncated: true,
            },
            &CapturedOutput {
                bytes: "输出"
                    .repeat(REMOTE_STREAM_OUTPUT_BYTES as usize / 6)
                    .into_bytes(),
                truncated: false,
            },
        );
        assert!(result.len() < REMOTE_RESULT_OUTPUT_BYTES + 64);
        assert!(result.ends_with("[输出已截断]"));
        assert!(!result.contains('\u{fffd}'));
    }

    #[cfg(unix)]
    #[test]
    fn verbose_remote_commands_finish_after_output_is_truncated() {
        for command in [
            "set -e; head -c 1048576 /dev/zero; printf completed >&2",
            "set -e; head -c 1048576 /dev/zero >&2; printf completed",
        ] {
            let result = execute_remote_task_with_timeout(
                &RemoteTaskMessage {
                    message_type: "remote_task".to_string(),
                    task_id: "verbose-output".to_string(),
                    command: command.to_string(),
                },
                Duration::from_secs(5),
            );
            assert_eq!(result.status, "success");
            assert_eq!(result.exit_code, Some(0));
            assert!(result.result.contains("completed"));
            assert!(result.result.ends_with("[输出已截断]"));
            assert!(result.result.len() < REMOTE_RESULT_OUTPUT_BYTES + 64);
        }
    }

    fn temporary_remote_task_journal(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "nodeflare-agent-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        directory.join("remote-tasks.json")
    }

    #[test]
    fn interrupted_remote_tasks_are_failed_without_reexecution() {
        let path = temporary_remote_task_journal("interrupted-task");
        let task_id = "8dd70536-f721-4d47-af63-e2f17c83de25";
        write_remote_task_journal(
            &path,
            vec![RemoteTaskJournalEntry::Active {
                task_id: task_id.to_string(),
            }],
        )
        .unwrap();

        let mut executor = RemoteExecutor::new_at(path.clone()).unwrap();
        assert!(executor.active.is_empty());
        let due = executor.due_results();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].task_id, task_id);
        assert_eq!(due[0].status, "failed");
        assert!(due[0].result.contains("不会再次执行"));

        executor.mark_sent(task_id);
        assert!(executor.due_results().is_empty());
        executor.reset_delivery();
        assert_eq!(executor.due_results().len(), 1);
        executor.acknowledge(task_id);
        assert!(!path.exists());

        let directory = path.parent().unwrap().to_path_buf();
        drop(executor);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completed_remote_tasks_return_the_cached_result() {
        let path = temporary_remote_task_journal("completed-task");
        let task_id = "6cbf4d36-f33a-40ef-b952-6af1b8491820";
        let result = TaskResultMessage {
            message_type: "task_result".to_string(),
            task_id: task_id.to_string(),
            status: "success".to_string(),
            result: "already finished".to_string(),
            exit_code: Some(0),
        };
        write_remote_task_journal(
            &path,
            vec![RemoteTaskJournalEntry::Completed {
                result: result.clone(),
            }],
        )
        .unwrap();

        let mut executor = RemoteExecutor::new_at(path.clone()).unwrap();
        assert!(
            executor
                .enqueue(RemoteTaskMessage {
                    message_type: "remote_task".to_string(),
                    task_id: task_id.to_string(),
                    command: "this command must not run".to_string(),
                })
                .is_none()
        );
        assert!(executor.active.is_empty());
        assert_eq!(executor.due_results(), [result]);
        executor.acknowledge(task_id);

        let directory = path.parent().unwrap().to_path_buf();
        drop(executor);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn encodes_realtime_samples_as_a_batch() {
        let payload = live_update_payload(
            vec![
                Report {
                    timestamp: 10,
                    cpu: 20.0,
                    ..Report::default()
                },
                Report {
                    timestamp: 15,
                    cpu: 30.0,
                    ..Report::default()
                },
            ],
            false,
            &mut None,
        )
        .unwrap();
        let value: serde_json::Value = nodeflare_telemetry::decode(&payload).unwrap();
        assert_eq!(value["persist"], false);
        assert_eq!(value["samples"].as_array().unwrap().len(), 2);
        assert_eq!(value["samples"][1]["metrics"]["cpu"], 30.0);
    }

    #[test]
    fn calibrates_timestamps_from_http_date() {
        let server_ms = 784_111_777_000_i64;
        assert_eq!(
            clock_offset_from_http_date(
                "Sun, 06 Nov 1994 08:49:37 GMT",
                server_ms - 1_000,
                server_ms + 1_000,
            ),
            Some(0),
        );
        assert_eq!(corrected_timestamp(100, 2_500), 102);
        assert_eq!(corrected_timestamp(100, -2_500), 97);
        assert_eq!(monotonic_report_timestamp(97, 100, 90), 101);
        assert_eq!(monotonic_report_timestamp(110, 100, 120), 121);
        assert_eq!(monotonic_report_timestamp(130, 120, 110), 130);
    }

    #[test]
    fn ignores_small_clock_jitter_until_the_calibration_expires() {
        let mut calibration = ClockCalibration::default();
        assert!(calibration.observe(1_000));
        assert!(!calibration.observe(2_000));
        assert_eq!(calibration.offset_ms, 1_000);
        assert!(calibration.observe(25_000));
        calibration.calibrated_at = Some(Instant::now() - CLOCK_CALIBRATION_MAX_AGE);
        assert!(calibration.observe(25_001));
    }

    #[test]
    fn parses_cli_options() {
        let parsed = CliOptions::try_parse_from([
            "agent",
            "-e",
            "https://monitor.example.com",
            "-t",
            "agent-token",
            "-i",
            "60",
            "--once",
        ])
        .unwrap();
        assert_eq!(parsed.endpoint, "https://monitor.example.com");
        assert_eq!(parsed.token.as_deref(), Some("agent-token"));
        assert_eq!(parsed.interval, 60);
        assert!(parsed.once);
        assert!(CliOptions::try_parse_from(["agent", "-t"]).is_err());
        assert!(CliOptions::try_parse_from(["agent", "-t", "first", "-t", "second"]).is_err());
        assert!(
            CliOptions::try_parse_from([
                "agent",
                "-e",
                "https://monitor.example.com",
                "-t",
                "agent-token",
                "--token-file",
                "/run/nodeflare/token",
            ])
            .is_err()
        );
        assert!(CliOptions::try_parse_from(["agent", "--once", "--collect"]).is_err());
    }

    #[test]
    fn filters_and_deduplicates_remote_latency_tasks_without_a_count_limit() {
        let valid = LatencyTask {
            id: "task-1".to_string(),
            name: "Cloudflare".to_string(),
            task_type: "tcp".to_string(),
            target: "1.1.1.1".to_string(),
            port: Some(443),
            interval_seconds: 60,
        };
        let mut tasks = vec![valid.clone(), valid];
        tasks.push(LatencyTask {
            id: "bad".to_string(),
            name: "Bad".to_string(),
            task_type: "http".to_string(),
            target: "https://example.com".to_string(),
            port: Some(443),
            interval_seconds: 1,
        });
        for index in 2..=300 {
            tasks.push(LatencyTask {
                id: format!("task-{index}"),
                name: format!("Task {index}"),
                task_type: "icmp".to_string(),
                target: "1.1.1.1".to_string(),
                port: None,
                interval_seconds: 60,
            });
        }
        let sanitized = sanitize_latency_tasks(&tasks);
        assert_eq!(sanitized.len(), 300);
        assert_eq!(sanitized[0].id, "task-1");
        assert!(!sanitized.iter().any(|task| task.id == "bad"));
    }

    #[test]
    fn advances_collection_deadlines_without_drift() {
        let start = Instant::now();
        let interval = Duration::from_secs(1);
        assert_eq!(
            advance_deadline(start, interval, start + Duration::from_millis(2500)),
            start + Duration::from_secs(3)
        );
    }

    #[test]
    fn drops_samples_the_server_would_reject_as_expired() {
        let mut samples = vec![
            Report {
                timestamp: 1_000,
                ..Report::default()
            },
            Report {
                timestamp: 8_500,
                ..Report::default()
            },
        ];
        prune_report_samples(&mut samples, 9_000);
        assert_eq!(samples.len(), 1);
        assert_eq!(samples[0].timestamp, 8_500);
    }

    #[test]
    fn bounds_pending_latency_results() {
        let mut samples = (0..MAX_PENDING_LATENCY_RESULTS + 10)
            .map(|index| Report {
                timestamp: index as i64 + 1,
                latency_results: vec![LatencyResult {
                    task_id: format!("task-{index}"),
                    timestamp: index as i64 + 1,
                    latency_ms: 10.0,
                    packet_loss: 0.0,
                }],
                ..Report::default()
            })
            .collect::<Vec<_>>();
        prune_report_samples(&mut samples, MAX_PENDING_LATENCY_RESULTS as i64 + 10);
        assert_eq!(
            samples
                .iter()
                .map(|sample| sample.latency_results.len())
                .sum::<usize>(),
            MAX_PENDING_LATENCY_RESULTS
        );
        assert!(samples.first().unwrap().latency_results.is_empty());
        assert_eq!(samples.last().unwrap().latency_results.len(), 1);
    }

    #[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
    #[test]
    fn parses_netstat_connection_counts() {
        let output = "tcp4 0 0 host.443 peer.1 ESTABLISHED\nudp4 0 0 *.5353 *.*\nTCP host peer ESTABLISHED\n";
        assert_eq!(connection_counts_from_netstat(output), (2, 1));
    }

    #[test]
    fn parses_probe_targets() {
        assert_eq!(
            parse_probe_target("Example.COM", None),
            Some(("example.com".to_string(), 443))
        );
        assert_eq!(
            parse_probe_target("1.1.1.1", Some(8080)),
            Some(("1.1.1.1".to_string(), 8080))
        );
        for target in [
            "",
            "https://example.com",
            "example.com:0",
            "example.com:65536",
            "999.1.1.1",
            "127.0.0.1:8080",
            "169.254.169.254",
            "192.168.1.1",
            "router.local",
            "localhost",
            "[::1]:443",
            "bad host",
        ] {
            assert_eq!(parse_probe_target(target, None), None, "target: {target}");
        }
    }

    #[test]
    fn accepts_only_public_probe_addresses() {
        for address in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(is_public_probe_ip(address.parse().expect("IP address")));
        }
        for address in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fe80::1",
            "fd00:ec2::254",
        ] {
            assert!(!is_public_probe_ip(address.parse().expect("IP address")));
        }
    }

    #[test]
    fn parses_ping_latency() {
        assert_eq!(ping_latency("64 bytes time=12.34 ms"), Some(12.34));
        assert_eq!(ping_latency("64 bytes time<1 ms"), Some(0.5));
        assert_eq!(ping_latency("unreachable"), None);
        assert_eq!(
            ping_latencies("reply time=12.34 ms\nrequest timeout\nreply time=8.5 ms"),
            vec![12.34, 8.5]
        );
    }

    #[test]
    fn update_check_jitter_is_stable_and_bounded() {
        let first = update_check_jitter("agent-token");
        assert_eq!(first, update_check_jitter("agent-token"));
        assert!(first.as_secs() <= UPDATE_CHECK_JITTER_MAX_SECONDS);
    }
}
