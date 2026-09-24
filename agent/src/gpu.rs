//! GPU discovery and usage sampling.
//!
//! `basic_gpu_info` has one implementation per supported platform, so this
//! module keeps the `cfg`-gated variants together with the shared parsers.

use super::*;

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
