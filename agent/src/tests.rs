use clap::Parser;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{
    CLOCK_CALIBRATION_MAX_AGE, CapturedOutput, CliOptions, ClockCalibration, DiskMetric,
    GithubReleaseAsset, LatencyResult, LatencyTask, LiveAck, MAX_PENDING_LATENCY_RESULTS,
    PUBLIC_IP_STALE_AFTER, PublicIpValue, REMOTE_RESULT_OUTPUT_BYTES, REMOTE_STREAM_OUTPUT_BYTES,
    RemoteExecutor, RemoteTaskJournalEntry, RemoteTaskMessage, Report, TaskResultMessage,
    UPDATE_CHECK_JITTER_MAX_SECONDS, ack_persist_interval, advance_deadline,
    clock_offset_from_http_date, corrected_timestamp, dedupe_disks,
    execute_remote_task_with_timeout, gpu_name_from_uevent, is_public_probe_ip, live_endpoint,
    live_update_payload, monotonic_report_timestamp, normalized_version, parse_lspci_gpu_names,
    parse_pciconf_gpu_names, parse_probe_target, parse_public_ip, parse_system_profiler_gpu_names,
    ping_latencies, ping_latency, prune_report_samples, release_asset_sha256, remote_result_text,
    sanitize_latency_tasks, selected_interface, update_check_jitter, valid_endpoint,
    version_triplet, wildcard_match, write_remote_task_journal,
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
fn filters_virtual_interfaces_and_supports_include_exclude_patterns() {
    assert!(!selected_interface("lo", ""));
    assert!(!selected_interface("docker0", ""));
    assert!(!selected_interface("veth123", ""));
    assert!(!selected_interface("br0", "!eth*"));
    assert!(selected_interface("br0", "br0"));
    assert!(!selected_interface("br0", "br*,!br0"));
    assert!(!selected_interface("br0", "!br0,br*"));
    assert!(!selected_interface("Software Loopback Interface 1", ""));
    assert!(selected_interface("eth0", ""));
    assert!(selected_interface("ens3", "eth*,ens*"));
    assert!(!selected_interface("wlan0", "eth*,ens*"));
    assert!(!selected_interface("eth0", "eth*,!eth0"));
    assert!(selected_interface("ens3", "!eth*"));
    assert!(wildcard_match("en?3", "ens3"));
    assert!(!wildcard_match("en?3", "enp4s0"));
}

#[test]
fn deduplicates_mounts_for_the_same_device() {
    let disks = dedupe_disks([
        DiskMetric {
            name: "/dev/vg/root".into(),
            mount_point: "/var".into(),
            total: 10,
            used: 4,
            ..DiskMetric::default()
        },
        DiskMetric {
            name: "/dev/vg/root".into(),
            mount_point: "/".into(),
            total: 20,
            used: 8,
            ..DiskMetric::default()
        },
        DiskMetric {
            name: "tank/data".into(),
            mount_point: "/data".into(),
            total: 30,
            used: 9,
            ..DiskMetric::default()
        },
        DiskMetric {
            name: "tank/archive".into(),
            mount_point: "/archive".into(),
            total: 25,
            used: 7,
            ..DiskMetric::default()
        },
        DiskMetric {
            name: "/var/lib/data".into(),
            mount_point: "/var/lib/data".into(),
            total: 40,
            used: 12,
            ..DiskMetric::default()
        },
    ]);
    assert_eq!(disks.len(), 3);
    assert_eq!(disks[0].mount_point, "/");
    assert_eq!(disks[0].total, 20);
    assert_eq!(disks[1].name, "tank/data");
    assert_eq!(disks[2].name, "/var/lib/data");
}

#[cfg(target_os = "linux")]
#[test]
fn keeps_root_filesystem_but_filters_virtual_mounts() {
    assert!(!super::excluded_filesystem("overlay", "/"));
    assert!(super::excluded_filesystem("tmpfs", "/tmp"));
    assert!(super::excluded_filesystem("proc", "/proc"));
    assert!(!super::excluded_filesystem("btrfs", "/var"));
}

#[cfg(target_os = "linux")]
#[test]
fn memory_matches_komari_default_including_shared_memory_and_swap_cache() {
    let memory = super::memory_sample(
        "MemTotal: 1000000 kB\nMemFree: 100000 kB\nMemAvailable: 750000 kB\n\
         Cached: 300000 kB\nSReclaimable: 50000 kB\nBuffers: 25000 kB\n\
         Shmem: 25000 kB\nSwapTotal: 100000 kB\nSwapFree: 60000 kB\nSwapCached: 20000 kB\n",
    );
    assert_eq!(memory.total, 1_000_000 * 1024);
    assert_eq!(memory.used, 550_000 * 1024);
    assert_eq!(memory.swap_total, 100_000 * 1024);
    assert_eq!(memory.swap_used, 20_000 * 1024);
}

#[cfg(target_os = "linux")]
#[test]
fn filling_file_cache_does_not_inflate_reported_memory() {
    let before = super::memory_sample(
        "MemTotal: 1000 kB\nMemFree: 400 kB\nCached: 300 kB\n\
         Buffers: 50 kB\nSReclaimable: 50 kB\nShmem: 100 kB\n",
    );
    let after = super::memory_sample(
        "MemTotal: 1000 kB\nMemFree: 200 kB\nCached: 500 kB\n\
         Buffers: 50 kB\nSReclaimable: 50 kB\nShmem: 100 kB\n",
    );
    assert_eq!(before.used, 300 * 1024);
    assert_eq!(after.used, before.used);
}

#[cfg(target_os = "linux")]
#[test]
fn memory_handles_missing_and_inconsistent_kernel_counters() {
    let memory = super::memory_sample(
        "MemTotal: 1000 kB\nMemFree: 200 kB\nCached: 1000 kB\n\
         Shmem: 100 kB\nSwapTotal: 100 kB\nSwapFree: 20 kB\nSwapCached: 100 kB\n",
    );
    assert_eq!(memory.used, 900 * 1024);
    assert_eq!(memory.swap_used, 80 * 1024);
    for contents in ["", "MemTotal: invalid kB\n", "MemTotal: -1 kB\n"] {
        let memory = super::memory_sample(contents);
        assert_eq!(
            (
                memory.total,
                memory.used,
                memory.swap_total,
                memory.swap_used
            ),
            (0, 0, 0, 0)
        );
    }
    let memory = super::memory_sample(
        "MemTotal: 100 kB\nMemFree: 1000 kB\nSwapTotal: 100 kB\nSwapFree: 1000 kB\n",
    );
    assert_eq!((memory.used, memory.swap_used), (0, 0));
}

#[cfg(target_os = "linux")]
#[test]
fn reads_local_mounts_and_decodes_escaped_paths_once() {
    let mounts = super::disk_mounts(
        r"overlay / overlay rw 0 0
proc /proc proc rw 0 0
tmpfs /run tmpfs rw 0 0
/dev/vda1 /data\040volume ext4 rw 0 0
/dev/vda2 /literal\134040path ext4 rw 0 0
/dev/vda3 /tab\011dir ext4 rw 0 0
server:/export /remote nfs4 rw 0 0
//server/share /remote-share cifs rw 0 0
none /auto autofs rw 0 0
invalid",
    )
    .collect::<Vec<_>>();
    assert_eq!(
        mounts,
        vec![
            ("overlay".into(), "/".into()),
            ("/dev/vda1".into(), "/data volume".into()),
            ("/dev/vda2".into(), "/literal\\040path".into()),
            ("/dev/vda3".into(), "/tab\tdir".into()),
        ]
    );
}

#[cfg(target_os = "linux")]
#[test]
fn reads_root_disk_capacity_without_external_commands() {
    let (total, used) = super::filesystem_space("/").unwrap();
    assert!(total > 0);
    assert!((0..=total).contains(&used));
    assert!(
        super::disk_usage()
            .iter()
            .any(|disk| disk.mount_point == "/")
    );
    assert!(super::filesystem_space("/proc/self/not-a-mount").is_none());
    assert!(super::filesystem_space("/nul\0path").is_none());
}

#[test]
fn overdue_latency_tasks_wait_and_resume_without_duplicate_probes() {
    let (task_tx, task_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let mut executor = super::LatencyExecutor {
        task_tx,
        result_rx,
        in_flight: Default::default(),
        stats: Default::default(),
    };
    let task = LatencyTask {
        id: "queued-probe".into(),
        name: "probe".into(),
        task_type: "tcp".into(),
        target: "example.com".into(),
        port: Some(443),
        interval_seconds: 30,
    };
    let mut deadlines = std::collections::HashMap::new();
    let started = Instant::now();
    executor.schedule(std::slice::from_ref(&task), &mut deadlines, started);
    assert_eq!(task_rx.try_recv().unwrap().id, task.id);
    assert_eq!(deadlines[&task.id], started + Duration::from_secs(30));

    let overdue = started + Duration::from_secs(30);
    executor.schedule(std::slice::from_ref(&task), &mut deadlines, overdue);
    assert!(task_rx.try_recv().is_err());
    assert_eq!(deadlines[&task.id], overdue + Duration::from_secs(1));

    result_tx
        .send(LatencyResult {
            task_id: task.id.clone(),
            ..LatencyResult::default()
        })
        .unwrap();
    assert_eq!(executor.drain().len(), 1);
    let retry_at = deadlines[&task.id];
    executor.schedule(std::slice::from_ref(&task), &mut deadlines, retry_at);
    executor.schedule(std::slice::from_ref(&task), &mut deadlines, retry_at);
    assert_eq!(task_rx.try_recv().unwrap().id, task.id);
    assert!(task_rx.try_recv().is_err());
    assert_eq!(deadlines[&task.id], retry_at + Duration::from_secs(30));

    drop(task_rx);
    executor.in_flight.clear();
    let retry_at = deadlines[&task.id];
    executor.schedule(std::slice::from_ref(&task), &mut deadlines, retry_at);
    assert_eq!(deadlines[&task.id], retry_at + Duration::from_secs(1));
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
    let output =
        "tcp4 0 0 host.443 peer.1 ESTABLISHED\nudp4 0 0 *.5353 *.*\nTCP host peer ESTABLISHED\n";
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
