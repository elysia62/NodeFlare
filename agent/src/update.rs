use super::*;

#[derive(Debug, Deserialize)]
pub(crate) struct GithubRelease {
    pub(crate) tag_name: String,
    pub(crate) assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GithubReleaseAsset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) digest: Option<String>,
}

pub(crate) fn normalized_version(value: &str) -> &str {
    let value = value.trim();
    value.strip_prefix('v').unwrap_or(value)
}

pub(crate) fn version_triplet(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = normalized_version(value).split('.');
    let version = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(version)
}

pub(crate) fn agent_artifact_name() -> Option<&'static str> {
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

pub(crate) fn executable_format_valid(path: &Path) -> bool {
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

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
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

pub(crate) fn release_asset_sha256(asset: &GithubReleaseAsset) -> Option<String> {
    let digest = asset.digest.as_deref()?.strip_prefix("sha256:")?;
    (digest.len() == 64
        && digest
            .chars()
            .all(|character| character.is_ascii_hexdigit()))
    .then(|| digest.to_ascii_lowercase())
}

/// Downloads a candidate Agent binary and rejects it unless it is runnable and
/// byte-identical to the published asset. Every rejection carries its own reason
/// so a slow or blocked mirror is not misreported as a version mismatch.
pub(crate) fn download_agent(
    agent: &ureq::Agent,
    url: &str,
    destination: &Path,
    expected_version: &str,
    expected_sha256: &str,
) -> Result<()> {
    let response = agent
        .get(url)
        // The agent-wide 20s budget is sized for the small release metadata
        // request; a multi-megabyte binary needs more. Every phase is bounded
        // separately so a stalled mirror still fails fast:
        //   - connect / response headers: short, matching the agent-wide budget
        //   - body transfer: the larger allowance, which is the point of the fix
        // The global timeout also has to be raised, because ureq takes the
        // smallest applicable deadline and the inherited one would otherwise
        // cut the transfer off at 20s no matter what recv_body says.
        .config()
        .timeout_global(Some(AGENT_DOWNLOAD_TIMEOUT))
        .timeout_connect(Some(AGENT_DOWNLOAD_HANDSHAKE_TIMEOUT))
        .timeout_recv_response(Some(AGENT_DOWNLOAD_HANDSHAKE_TIMEOUT))
        .timeout_recv_body(Some(AGENT_DOWNLOAD_TIMEOUT))
        .build()
        .call()
        .map_err(|error| format!("download from {url} failed: {error}"))?;
    let response = response.into_body().into_reader();
    let mut file = fs::File::create(destination)?;
    let copied = io::copy(&mut response.take(MAX_AGENT_BINARY_BYTES + 1), &mut file)?;
    file.flush()?;
    drop(file);
    if copied > MAX_AGENT_BINARY_BYTES {
        let _ = fs::remove_file(destination);
        return Err(format!("download from {url} exceeded {MAX_AGENT_BINARY_BYTES} bytes").into());
    }
    if !executable_format_valid(destination) {
        let _ = fs::remove_file(destination);
        return Err(format!("download from {url} is not an executable for this system").into());
    }
    let actual_sha256 = sha256_file(destination)?;
    if actual_sha256 != expected_sha256 {
        let _ = fs::remove_file(destination);
        return Err(format!(
            "download from {url} is corrupted: sha256 {actual_sha256}, expected {expected_sha256}"
        )
        .into());
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
        return Err(format!(
            "download from {url} reports version {}, expected {expected_version}",
            downloaded_version.as_deref().unwrap_or("unknown")
        )
        .into());
    }
    Ok(())
}

/// Prefixes a GitHub URL with the configured mirror. Everything the Agent
/// fetches from GitHub goes through here, release metadata included: a host
/// that cannot reach GitHub would otherwise stall on the first request that
/// skips the mirror.
pub(crate) fn mirrored_url(mirror: &str, url: &str) -> String {
    let mirror = mirror.trim().trim_end_matches('/');
    if mirror.is_empty() {
        url.to_string()
    } else {
        format!("{mirror}/{url}")
    }
}

/// The release metadata endpoint, already routed through the mirror. Kept apart
/// from `update` so the mirror binding is covered by a test: this call is the
/// first GitHub request an updating Agent makes, and skipping the mirror here
/// strands hosts that cannot reach GitHub.
pub(crate) fn release_metadata_url(mirror: &str) -> String {
    mirrored_url(mirror, LATEST_RELEASE_API)
}

pub(crate) fn update(agent: &ureq::Agent, mirror: &str) -> Result<bool> {
    let mut response = agent
        .get(&release_metadata_url(mirror))
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
    download_agent(
        agent,
        &mirrored_url(mirror, &release_asset.browser_download_url),
        &temporary,
        &release.tag_name,
        &expected_sha256,
    )?;

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
