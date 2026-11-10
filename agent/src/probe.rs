//! Latency probing: target validation, resolution and TCP/ICMP measurement.
//!
//! Kept free of `cfg`-gated items so the same probes run on every platform the
//! Agent supports.

use super::*;

pub(crate) use telemetry::probe::{is_public_probe_ip, valid_probe_host};

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
    let mut latencies = match ping.output() {
        Ok(output) => ping_latencies(&String::from_utf8_lossy(&output.stdout)),
        Err(error) => {
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                eprintln!("ICMP probe unavailable: cannot execute ping: {error}");
            }
            return (-1.0, -1.0);
        }
    };
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
