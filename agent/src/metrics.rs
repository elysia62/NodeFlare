use super::*;

#[derive(Debug, Default, Clone)]
pub(crate) struct BasicMetrics {
    pub(crate) cpu_cores: i64,
    pub(crate) cpu_model: String,
    pub(crate) os: String,
    pub(crate) kernel: String,
    pub(crate) arch: String,
    pub(crate) virtualization: String,
    pub(crate) gpu_usage: f64,
    pub(crate) gpu_model: String,
    pub(crate) gpus: Vec<GpuMetric>,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct SlowMetrics {
    pub(crate) disks: Vec<DiskMetric>,
    pub(crate) processes: i64,
    pub(crate) tcp_connections: i64,
    pub(crate) udp_connections: i64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct CpuSample {
    pub(crate) total: u64,
    pub(crate) idle: u64,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct IoSample {
    pub(crate) rx: u64,
    pub(crate) tx: u64,
    pub(crate) read_ops: u64,
    pub(crate) read_sectors: u64,
    pub(crate) write_ops: u64,
    pub(crate) write_sectors: u64,
    pub(crate) read_millis: u64,
    pub(crate) write_millis: u64,
    pub(crate) io_millis: u64,
}

#[cfg(target_os = "linux")]
pub(crate) fn text(path: impl AsRef<Path>) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

/// Runs a command and captures stdout, killing it after `timeout`.
///
/// A dedicated thread drains the pipe while we poll, so a command whose output
/// exceeds the pipe buffer is not mistaken for a hang. Without the timeout a
/// wedged child (a stuck GPU driver, an unresponsive netstat) would block the
/// sampler thread forever and silently stop telemetry.
pub(crate) fn output_with_timeout(mut process: Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = process
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let reader = child.stdout.take().map(|mut pipe| {
        thread::spawn(move || {
            let mut buffer = Vec::new();
            let _ = pipe.read_to_end(&mut buffer);
            buffer
        })
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = reader
        .and_then(|handle| handle.join().ok())
        .unwrap_or_default();
    Some(std::process::Output {
        status: status?,
        stdout,
        stderr: Vec::new(),
    })
}

pub(crate) fn command(name: &str, args: &[&str]) -> String {
    let mut process = Command::new(name);
    process.args(args);
    output_with_timeout(process, COMMAND_TIMEOUT)
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
pub(crate) fn cpu_sample() -> CpuSample {
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

pub(crate) fn selected_interface(name: &str, filter: &str) -> bool {
    let name = name.trim();
    if name.is_empty() {
        return false;
    }
    let mut has_include = false;
    let mut included = false;
    for pattern in filter
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(pattern) = pattern
            .strip_prefix('!')
            .or_else(|| pattern.strip_prefix('-'))
        {
            if !pattern.is_empty() && wildcard_match(pattern, name) {
                return false;
            }
        } else {
            has_include = true;
            included |= wildcard_match(pattern, name);
        }
    }
    // Explicit includes can select a bridge; automatic selection avoids double counting.
    if has_include {
        return included;
    }
    let lower = name.to_ascii_lowercase();
    !lower.contains("loopback")
        && ![
            "lo", "br", "cni", "docker", "podman", "flannel", "veth", "virbr", "vmbr", "tap",
            "fwbr", "fwpr",
        ]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

pub(crate) fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let (mut p, mut v, mut star, mut star_value) = (0, 0, None, 0);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            star_value = v;
            p += 1;
        } else if let Some(star_position) = star {
            p = star_position + 1;
            star_value += 1;
            v = star_value;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(target_os = "linux")]
pub(crate) fn disk_device(name: &str) -> bool {
    ((name.starts_with("sd") || name.starts_with("vd") || name.starts_with("xvd"))
        && name
            .chars()
            .last()
            .is_some_and(|value| value.is_ascii_alphabetic()))
        || (name.starts_with("nvme") && name.contains('n') && !name.contains('p'))
        || (name.starts_with("mmcblk") && !name.contains('p'))
}

#[cfg(target_os = "linux")]
pub(crate) fn io_sample(filter: &str) -> IoSample {
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
pub(crate) fn mem_value(contents: &str, key: &str) -> i64 {
    contents
        .lines()
        .find(|line| line.starts_with(key))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u64>().ok())
        .map_or(0, |value| u64_to_i64(value.saturating_mul(1024)))
}

#[cfg(target_os = "linux")]
pub(crate) struct MemorySample {
    pub(crate) total: i64,
    pub(crate) used: i64,
    pub(crate) swap_total: i64,
    pub(crate) swap_used: i64,
}

#[cfg(target_os = "linux")]
pub(crate) fn memory_sample(contents: &str) -> MemorySample {
    let total = mem_value(contents, "MemTotal:");
    let free = mem_value(contents, "MemFree:");
    let reclaimable = free
        .saturating_add(mem_value(contents, "Cached:"))
        .saturating_add(mem_value(contents, "SReclaimable:"))
        .saturating_add(mem_value(contents, "Buffers:"));
    // Komari's default Linux/htop calculation excludes reclaimable caches but
    // includes shared memory. MemAvailable uses a different kernel estimate.
    let used = total
        .saturating_sub(if reclaimable <= total {
            reclaimable
        } else {
            free
        })
        .saturating_add(mem_value(contents, "Shmem:"))
        .clamp(0, total);
    let swap_total = mem_value(contents, "SwapTotal:");
    let swap_free = mem_value(contents, "SwapFree:");
    let swap_deductions = swap_free.saturating_add(mem_value(contents, "SwapCached:"));
    let swap_used = swap_total
        .saturating_sub(if swap_deductions <= swap_total {
            swap_deductions
        } else {
            swap_free
        })
        .clamp(0, swap_total);
    MemorySample {
        total,
        used,
        swap_total,
        swap_used,
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn file_line_count(path: &str) -> i64 {
    text(path).lines().skip(1).count() as i64
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
pub(crate) fn connection_counts_from_netstat(output: &str) -> (i64, i64) {
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
pub(crate) fn connection_counts() -> (i64, i64) {
    let mut process = Command::new("netstat");
    process.args(["-an"]);
    output_with_timeout(process, COMMAND_TIMEOUT)
        .filter(|output| output.status.success())
        .map(|output| connection_counts_from_netstat(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
pub(crate) fn disk_usage() -> Vec<DiskMetric> {
    disk_mounts(&text("/proc/self/mounts"))
        .filter_map(|(name, mount_point)| {
            // Ignore file bind mounts such as a container's /etc/hosts.
            if !Path::new(&mount_point).is_dir() {
                return None;
            }
            let (total, used) = filesystem_space(&mount_point)?;
            Some(DiskMetric {
                name,
                mount_point,
                used,
                total,
                ..DiskMetric::default()
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub(crate) fn disk_mounts(contents: &str) -> impl Iterator<Item = (String, String)> + '_ {
    pub(crate) fn unescape(value: &str) -> String {
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\")
    }
    contents.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let source = unescape(fields.next()?);
        let mount_point = unescape(fields.next()?);
        let filesystem = fields.next()?;
        if (!source.starts_with('/') && source.contains(':'))
            || source.starts_with("//")
            || excluded_filesystem(filesystem, &mount_point)
        {
            return None;
        }
        Some((source, mount_point))
    })
}

#[cfg(target_os = "linux")]
pub(crate) fn filesystem_space(mount_point: &str) -> Option<(i64, i64)> {
    let path = std::ffi::CString::new(mount_point).ok()?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: path is NUL-terminated and stat points to writable storage of the
    // platform's statvfs type, including both glibc and musl layouts.
    if unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs returned success and initialized stat.
    let stat = unsafe { stat.assume_init() };
    let block_size = u128::from(if stat.f_frsize == 0 {
        stat.f_bsize
    } else {
        stat.f_frsize
    });
    let total = (u128::from(stat.f_blocks) * block_size).min(i64::MAX as u128) as i64;
    let used = (u128::from(stat.f_blocks.saturating_sub(stat.f_bfree)) * block_size)
        .min(i64::MAX as u128) as i64;
    (total > 0).then_some((total, used))
}

#[cfg(target_os = "linux")]
pub(crate) fn excluded_filesystem(filesystem: &str, mount_point: &str) -> bool {
    // Remote/autofs mounts can block indefinitely while their server is down.
    if matches!(
        filesystem,
        "nfs"
            | "nfs4"
            | "cifs"
            | "smb3"
            | "autofs"
            | "9p"
            | "ceph"
            | "afs"
            | "fuse.sshfs"
            | "fuse.glusterfs"
            | "fuse.rclone"
            | "fuse.s3fs"
    ) {
        return true;
    }
    let mount_point = mount_point.to_ascii_lowercase();
    // Keep the root filesystem visible even when a container reports it as overlay.
    if mount_point == "/" {
        return false;
    }
    let filesystem = filesystem.to_ascii_lowercase();
    [
        "tmpfs",
        "devtmpfs",
        "devpts",
        "proc",
        "sysfs",
        "cgroup",
        "cgroup2",
        "overlay",
        "squashfs",
        "efivarfs",
        "pstore",
        "mqueue",
        "hugetlbfs",
        "debugfs",
        "fusectl",
    ]
    .iter()
    .any(|value| filesystem == *value || filesystem.starts_with(&format!("{value}.")))
        || ["/proc", "/sys", "/run", "/dev"]
            .iter()
            .any(|prefix| mount_point == *prefix || mount_point.starts_with(&format!("{prefix}/")))
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn disk_identity(name: &str) -> String {
    let name = name.trim();
    // ZFS reports datasets as pool/dataset. Keep ordinary absolute paths and
    // remote sources separate because their slash is part of the path.
    if !name.starts_with('/') && !name.contains(':') && name.contains('/') {
        return name.split('/').next().unwrap_or(name).to_string();
    }
    name.to_string()
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn dedupe_disks(disks: impl IntoIterator<Item = DiskMetric>) -> Vec<DiskMetric> {
    let mut unique = HashMap::<String, DiskMetric>::new();
    for disk in disks {
        let key = disk_identity(&disk.name);
        let replace = unique.get(&key).is_none_or(|current| {
            disk.total > current.total
                || (disk.total == current.total
                    && disk.mount_point.len() < current.mount_point.len())
        });
        if replace {
            unique.insert(key, disk);
        }
    }
    let mut disks = unique.into_values().collect::<Vec<_>>();
    disks.sort_by(|left, right| left.mount_point.cmp(&right.mount_point));
    disks
}

#[cfg(target_os = "linux")]
pub(crate) fn os_name() -> String {
    text("/etc/os-release")
        .lines()
        .find_map(|line| line.strip_prefix("PRETTY_NAME="))
        .map_or_else(
            || command("uname", &["-s"]),
            |value| value.trim_matches('"').to_string(),
        )
}

#[cfg(target_os = "linux")]
pub(crate) fn cpu_model() -> String {
    text("/proc/cpuinfo")
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            matches!(key.trim(), "model name" | "Hardware").then(|| value.trim().to_string())
        })
        .unwrap_or_default()
}

pub(crate) fn valid_probe_host(host: &str) -> bool {
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

pub(crate) fn is_public_probe_ip(address: IpAddr) -> bool {
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

pub(crate) fn resolve_public_probe_address(host: &str, port: u16) -> Option<SocketAddr> {
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

pub(crate) fn parse_probe_target(value: &str, port: Option<u16>) -> Option<(String, u16)> {
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

pub(crate) fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

pub(crate) fn tcp_latency_probe(target: &str, port: Option<i64>) -> (f64, f64) {
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

pub(crate) fn tcp_latency_probe_address(address: &SocketAddr) -> (f64, f64) {
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

pub(crate) fn ping_latency(output: &str) -> Option<f64> {
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

pub(crate) fn ping_latencies(output: &str) -> Vec<f64> {
    output.lines().filter_map(ping_latency).collect()
}

pub(crate) fn icmp_latency_probe(target: &str) -> (f64, f64) {
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

pub(crate) fn execute_latency_task(task: LatencyTask) -> LatencyResult {
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

pub(crate) fn nvidia_gpu_info() -> Vec<GpuMetric> {
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

pub(crate) fn basic_gpu_metrics(names: impl IntoIterator<Item = String>) -> Vec<GpuMetric> {
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
pub(crate) fn parse_lspci_gpu_names(output: &str) -> Vec<String> {
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
pub(crate) fn gpu_name_from_uevent(output: &str) -> Option<String> {
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
pub(crate) fn sysfs_gpu_names() -> Vec<String> {
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
pub(crate) fn parse_system_profiler_gpu_names(output: &str) -> Vec<String> {
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
pub(crate) fn parse_pciconf_gpu_names(output: &str) -> Vec<String> {
    pub(crate) fn block_name(lines: &[&str]) -> Option<String> {
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
pub(crate) fn basic_gpu_info() -> Vec<GpuMetric> {
    let names = parse_lspci_gpu_names(&command("lspci", &[]));
    basic_gpu_metrics(if names.is_empty() {
        sysfs_gpu_names()
    } else {
        names
    })
}

#[cfg(target_os = "windows")]
pub(crate) fn basic_gpu_info() -> Vec<GpuMetric> {
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
pub(crate) fn basic_gpu_info() -> Vec<GpuMetric> {
    basic_gpu_metrics(parse_system_profiler_gpu_names(&command(
        "system_profiler",
        &["SPDisplaysDataType"],
    )))
}

#[cfg(target_os = "freebsd")]
pub(crate) fn basic_gpu_info() -> Vec<GpuMetric> {
    basic_gpu_metrics(parse_pciconf_gpu_names(&command("pciconf", &["-lv"])))
}

pub(crate) fn gpu_info() -> Vec<GpuMetric> {
    let detailed = nvidia_gpu_info();
    if detailed.is_empty() {
        basic_gpu_info()
    } else {
        detailed
    }
}

pub(crate) fn average_gpu_usage(gpus: &[GpuMetric]) -> f64 {
    let usages = gpus.iter().filter_map(|gpu| gpu.usage).collect::<Vec<_>>();
    if usages.is_empty() {
        0.0
    } else {
        usages.iter().sum::<f64>() / usages.len() as f64
    }
}

pub(crate) fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(crate) fn per_second(value: u64, elapsed_seconds: f64) -> f64 {
    value as f64 / elapsed_seconds.max(0.001)
}

#[cfg(target_os = "linux")]
pub(crate) struct Collector {
    pub(crate) previous_cpu: CpuSample,
    pub(crate) previous_io: IoSample,
    pub(crate) previous_at: Instant,
    pub(crate) network_interface: String,
    pub(crate) basic: BasicMetrics,
    pub(crate) basic_at: Instant,
    pub(crate) gpu_at: Instant,
    pub(crate) slow: SlowMetrics,
    pub(crate) slow_at: Instant,
    pub(crate) public_ip: PublicIpProbe,
}

#[cfg(target_os = "linux")]
impl Collector {
    pub(crate) fn new(config: &RuntimeConfig) -> Self {
        let mut collector = Self {
            previous_cpu: cpu_sample(),
            previous_io: io_sample(&config.network_interface),
            previous_at: Instant::now(),
            network_interface: config.network_interface.clone(),
            basic: BasicMetrics::default(),
            basic_at: Instant::now(),
            gpu_at: Instant::now(),
            slow: SlowMetrics::default(),
            slow_at: Instant::now(),
            public_ip: PublicIpProbe::default(),
        };
        collector.refresh_basic();
        collector.refresh_slow();
        collector
    }

    pub(crate) fn refresh_basic(&mut self) {
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
        self.gpu_at = Instant::now();
    }

    pub(crate) fn refresh_slow(&mut self) {
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
            disks: dedupe_disks(disk_usage()),
            processes,
            tcp_connections: file_line_count("/proc/net/tcp") + file_line_count("/proc/net/tcp6"),
            udp_connections: file_line_count("/proc/net/udp") + file_line_count("/proc/net/udp6"),
        };
        self.slow_at = Instant::now();
    }

    pub(crate) fn collect(
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
        if self.gpu_at.elapsed() >= GPU_METRICS_REFRESH_INTERVAL {
            self.refresh_gpu_metrics();
        }
        if self.slow_at.elapsed() >= SLOW_METRICS_REFRESH_INTERVAL {
            self.refresh_slow();
        }
        self.public_ip.refresh();

        let memory = memory_sample(&text("/proc/meminfo"));
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
            mem_used: memory.used,
            mem_total: memory.total,
            swap_used: memory.swap_used,
            swap_total: memory.swap_total,
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
pub(crate) struct Collector {
    pub(crate) system: System,
    pub(crate) networks: Networks,
    pub(crate) disks: Disks,
    pub(crate) previous_at: Instant,
    pub(crate) basic: BasicMetrics,
    pub(crate) basic_at: Instant,
    pub(crate) gpu_at: Instant,
    pub(crate) slow: SlowMetrics,
    pub(crate) slow_at: Instant,
    pub(crate) public_ip: PublicIpProbe,
}

#[cfg(any(target_os = "windows", target_os = "macos", target_os = "freebsd"))]
impl Collector {
    pub(crate) fn new(_config: &RuntimeConfig) -> Self {
        let mut collector = Self {
            system: System::new_all(),
            networks: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            previous_at: Instant::now(),
            basic: BasicMetrics::default(),
            basic_at: Instant::now(),
            gpu_at: Instant::now(),
            slow: SlowMetrics::default(),
            slow_at: Instant::now(),
            public_ip: PublicIpProbe::default(),
        };
        collector.refresh_basic();
        collector.refresh_slow();
        collector
    }

    pub(crate) fn refresh_basic(&mut self) {
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
        self.gpu_at = Instant::now();
    }

    pub(crate) fn refresh_slow(&mut self) {
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

    pub(crate) fn collect(
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
        if self.gpu_at.elapsed() >= GPU_METRICS_REFRESH_INTERVAL {
            self.refresh_gpu_metrics();
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
            mem_used: u64_to_i64(
                self.system
                    .total_memory()
                    .saturating_sub(self.system.available_memory()),
            ),
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

impl Collector {
    pub(crate) fn refresh_gpu_metrics(&mut self) {
        self.gpu_at = Instant::now();
        // Unsupported devices are rediscovered with basic info, not every sample.
        if !self
            .basic
            .gpus
            .iter()
            .any(|gpu| gpu.usage.is_some() || gpu.memory_total > 0)
        {
            return;
        }
        let detailed = nvidia_gpu_info();
        if detailed.is_empty() {
            return;
        }
        self.basic.gpu_usage = average_gpu_usage(&detailed);
        self.basic.gpu_model = detailed
            .iter()
            .map(|gpu| gpu.model.as_str())
            .collect::<Vec<_>>()
            .join(" · ");
        self.basic.gpus = detailed;
    }
}

