<#
.EXAMPLE
  .\install.ps1 -Update
  Update using the saved endpoint, token and history interval.
.EXAMPLE
  .\install.ps1 -Update -Mirror https://ghproxy.net
#>
[CmdletBinding(DefaultParameterSetName = "Install")]
param(
  [Parameter(ParameterSetName = "Install", Mandatory = $true)][Alias("t")][string]$Token,
  [Parameter(ParameterSetName = "Install", Mandatory = $true)][Alias("e")][string]$Endpoint,
  [Parameter(ParameterSetName = "Install")][Alias("i")][ValidateRange(15, 3600)][int]$Interval = 60,
  [Parameter(ParameterSetName = "Install")][Parameter(ParameterSetName = "Update")][Alias("m")][string]$Mirror = "",
  [Parameter(ParameterSetName = "Update", Mandatory = $true)][switch]$Update,
  [Parameter(ParameterSetName = "Uninstall", Mandatory = $true)][switch]$Uninstall,
  [Parameter(ParameterSetName = "Status", Mandatory = $true)][switch]$Status
)

$ErrorActionPreference = "Stop"
$TaskName = "nodeflare-agent"
$InstallDir = Join-Path $env:ProgramFiles "NodeFlare"
$DataDir = Join-Path $env:ProgramData "NodeFlare"
$StateDir = Join-Path $DataDir "Agent"
$AgentFile = Join-Path $InstallDir "agent.exe"
$ConfigFile = Join-Path $StateDir "config.json"
$LauncherFile = Join-Path $InstallDir "run-agent.ps1"
$PreviousAgent = Join-Path $env:TEMP "nodeflare-agent-$PID.previous.exe"
$PreviousConfig = Join-Path $env:TEMP "nodeflare-agent-$PID.previous.json"
$PreviousLauncher = Join-Path $env:TEMP "nodeflare-agent-$PID.previous.ps1"
$HadPreviousAgent = $false
$HadPreviousConfig = $false
$HadPreviousLauncher = $false
$PreviousTaskXml = $null
$InstallChanged = $false

function Write-Step([string]$Message) {
  Write-Host $Message
}

function Write-InstallError([string]$Message) {
  throw "错误：$Message"
}

function Show-InstallResult {
  Write-Host ""
  if ($Update -or $HadPreviousAgent) {
    Write-Host "更新完成（v$InstalledVersion）"
    return
  }
  Write-Host "安装完成（v$InstalledVersion）"
  Write-Host "  服务：$TaskName（Windows 计划任务）"
  Write-Host "  查看状态：Get-ScheduledTask -TaskName '$TaskName'"
}

function Assert-Safe([string]$Name, [string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[A-Za-z0-9_./:@-]+$') {
    Write-InstallError "$Name 格式无效"
  }
}

function Read-AgentConfig {
  if (-not (Test-Path -LiteralPath $ConfigFile -PathType Leaf)) {
    Write-InstallError "未找到已安装 Agent 的配置，请先安装"
  }
  try {
    $Saved = Get-Content -LiteralPath $ConfigFile -Raw | ConvertFrom-Json
  } catch {
    Write-InstallError "无法读取已安装 Agent 的配置"
  }
  $SavedInterval = 0
  if ($Saved.endpoint -isnot [string] -or $Saved.token -isnot [string] -or
      -not [int]::TryParse([string]$Saved.interval, [ref]$SavedInterval) -or
      $SavedInterval -lt 15 -or $SavedInterval -gt 3600) {
    Write-InstallError "已安装 Agent 的配置不完整或历史保存间隔无效"
  }
  return $Saved
}

function Assert-Endpoint([string]$Value) {
  $Parsed = $null
  $SecureScheme = $false
  if ([Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$Parsed)) {
    $SecureScheme = $Parsed.Scheme -eq "https" -or (
      $Parsed.Scheme -eq "http" -and $Parsed.Host -in @("localhost", "127.0.0.1", "::1")
    )
  }
  if (
    $Value.Length -gt 2048 -or
    $Value -match "\s" -or
    $Value.Contains("'") -or
    -not $SecureScheme -or
    -not [string]::IsNullOrEmpty($Parsed.UserInfo) -or
    -not [string]::IsNullOrEmpty($Parsed.Query) -or
    -not [string]::IsNullOrEmpty($Parsed.Fragment)
  ) {
    Write-InstallError "服务地址必须使用 HTTPS；仅本机调试可使用 HTTP"
  }
}

function Assert-Mirror([string]$Value) {
  $Parsed = $null
  $SecureScheme = $false
  if ([Uri]::TryCreate($Value, [UriKind]::Absolute, [ref]$Parsed)) {
    $SecureScheme = $Parsed.Scheme -eq "https" -or (
      $Parsed.Scheme -eq "http" -and $Parsed.Host -in @("localhost", "127.0.0.1", "::1")
    )
  }
  if (
    $Value -notmatch '^[A-Za-z0-9_./:@-]+$' -or
    -not $SecureScheme -or
    -not [string]::IsNullOrEmpty($Parsed.UserInfo)
  ) {
    Write-InstallError "下载加速前缀必须使用 HTTPS，且不能包含用户信息或查询参数"
  }
}

if ($Uninstall) {
  Write-Step "正在停止并移除 Agent 服务"
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $AgentFile -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $LauncherFile -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $StateDir -Recurse -Force -ErrorAction SilentlyContinue
  try { [IO.Directory]::Delete($DataDir, $false) } catch [IO.IOException] { }
  try { [IO.Directory]::Delete($InstallDir, $false) } catch [IO.IOException] { }
  Write-Host "Agent 已卸载"
  exit 0
}

if ($Status) {
  $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -eq $Task) {
    Write-Error "未检测到 Agent 服务"
    exit 1
  }
  $Task
  exit 0
}

if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  Write-InstallError "请使用管理员身份运行 PowerShell"
}
$NativeArchitecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($NativeArchitecture -ne "AMD64") {
  Write-InstallError "仅支持 Windows x64"
}
Write-Step "正在检查运行环境"
if ($Update) {
  $SavedConfig = Read-AgentConfig
  $Endpoint = $SavedConfig.endpoint
  $Token = $SavedConfig.token
  $Interval = [int]$SavedConfig.interval
}
Assert-Safe "Token" $Token
Assert-Endpoint $Endpoint
$Mirror = $Mirror.Trim().TrimEnd('/')
if ($Mirror) { Assert-Mirror $Mirror }
$TokenLength = $Token.Length
if ($TokenLength -gt 512) {
  Write-InstallError "安装参数长度超出限制"
}
$Endpoint = $Endpoint.TrimEnd('/')

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
New-Item -ItemType Directory -Path $DataDir -Force | Out-Null
New-Item -ItemType Directory -Path $StateDir -Force | Out-Null
& icacls.exe $InstallDir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { Write-InstallError "无法限制程序目录权限" }
& icacls.exe $StateDir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { Write-InstallError "无法限制 Agent 数据目录权限" }
$Temporary = "$AgentFile.$PID.download.exe"
$ReleaseApi = "https://api.github.com/repos/elysia62/NodeFlare/releases/latest"
$Artifact = "agent-windows-x64.exe"
try {
  Write-Step "正在获取 GitHub 最新正式版本（$Artifact）"
  $Release = Invoke-RestMethod -Uri $ReleaseApi -Headers @{ Accept = "application/vnd.github+json"; "User-Agent" = "nodeflare-installer" } -TimeoutSec 30
  $ReleaseAsset = $Release.assets | Where-Object { $_.name -eq $Artifact } | Select-Object -First 1
  if ($Release.tag_name -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$' -or $null -eq $ReleaseAsset) {
    Write-InstallError "GitHub 最新 Release 无效，或缺少 $Artifact"
  }
  $DigestMatch = [regex]::Match([string]$ReleaseAsset.digest, '^sha256:([0-9a-fA-F]{64})$')
  if (-not $DigestMatch.Success) {
    Write-InstallError "Release 缺少 $Artifact 的 SHA-256 摘要"
  }
  $ExpectedChecksum = $DigestMatch.Groups[1].Value
  $DownloadUrl = "https://github.com/elysia62/NodeFlare/releases/download/$($Release.tag_name)/$Artifact"
  if ($Mirror) {
    $DownloadUrl = "$Mirror/$DownloadUrl"
    Write-Step "正在通过下载加速前缀拉取 Agent $($Release.tag_name)"
  } else {
    Write-Step "正在下载 Agent $($Release.tag_name)"
  }
  Invoke-WebRequest -UseBasicParsing -Uri $DownloadUrl -OutFile $Temporary -TimeoutSec 120
  $ActualChecksum = (Get-FileHash -LiteralPath $Temporary -Algorithm SHA256).Hash
  if ($ActualChecksum -ne $ExpectedChecksum) {
    Write-InstallError "Agent SHA-256 校验失败，已停止安装"
  }
  Unblock-File -LiteralPath $Temporary
  Write-Step "下载校验通过，正在验证可执行文件"
  $InstalledVersion = (& $Temporary --version | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) {
    Write-InstallError "下载的 Agent 无法在当前 Windows 运行"
  }
  $InstalledVersion = ($InstalledVersion -split '\s+')[-1]
  $ExpectedVersion = $Release.tag_name.Substring(1)
  if ($InstalledVersion -ne $ExpectedVersion) {
    Write-InstallError "Release $($Release.tag_name) 与 Agent 版本 $InstalledVersion 不一致"
  }
  $HadPreviousAgent = Test-Path -LiteralPath $AgentFile -PathType Leaf
  $HadPreviousConfig = Test-Path -LiteralPath $ConfigFile -PathType Leaf
  $HadPreviousLauncher = Test-Path -LiteralPath $LauncherFile -PathType Leaf
  if ($HadPreviousAgent) { Copy-Item -LiteralPath $AgentFile -Destination $PreviousAgent -Force }
  if ($HadPreviousConfig) { Copy-Item -LiteralPath $ConfigFile -Destination $PreviousConfig -Force }
  if ($HadPreviousLauncher) { Copy-Item -LiteralPath $LauncherFile -Destination $PreviousLauncher -Force }
  $ExistingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -ne $ExistingTask) { $PreviousTaskXml = Export-ScheduledTask -TaskName $TaskName }
  $InstallChanged = $true
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Move-Item -LiteralPath $Temporary -Destination $AgentFile -Force

  $AgentConfig = [ordered]@{
    endpoint = $Endpoint
    token = $Token
    interval = $Interval
  }
  $AgentConfig | ConvertTo-Json -Compress | Set-Content -LiteralPath $ConfigFile -Encoding UTF8
  $Launcher = @'
$ErrorActionPreference = "Stop"
$InstallRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$AgentFile = Join-Path $InstallRoot "agent.exe"
$ConfigFile = Join-Path $env:ProgramData "NodeFlare\Agent\config.json"
$Config = Get-Content -LiteralPath $ConfigFile -Raw | ConvertFrom-Json
$env:NODEFLARE_AGENT_TOKEN = [string]$Config.token
& $AgentFile -e ([string]$Config.endpoint) -i ([int]$Config.interval)
exit $LASTEXITCODE
'@
  [IO.File]::WriteAllText($LauncherFile, $Launcher, [Text.UTF8Encoding]::new($false))
  & icacls.exe $ConfigFile /inheritance:r /grant:r '*S-1-5-18:F' '*S-1-5-32-544:F' | Out-Null
  if ($LASTEXITCODE -ne 0) { Write-InstallError "无法限制 Agent 配置文件权限" }
  & icacls.exe $LauncherFile /inheritance:r /grant:r '*S-1-5-18:F' '*S-1-5-32-544:F' | Out-Null
  if ($LASTEXITCODE -ne 0) { Write-InstallError "无法限制 Agent 启动脚本权限" }
  $Token = ""
  $PowerShell = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
  $TaskArguments = "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$LauncherFile`""
  Write-Step "正在注册并启动 Windows 计划任务"
  $TaskAction = New-ScheduledTaskAction -Execute $PowerShell -Argument $TaskArguments
  $Trigger = New-ScheduledTaskTrigger -AtStartup
  $Settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Days 3650)
  $Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
  Register-ScheduledTask -TaskName $TaskName -Action $TaskAction -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
  Start-ScheduledTask -TaskName $TaskName
  $Task = $null
  for ($Attempt = 0; $Attempt -lt 10; $Attempt++) {
    Start-Sleep -Milliseconds 500
    $Task = Get-ScheduledTask -TaskName $TaskName
    if ($Task.State -eq "Running") { break }
  }
  if ($Task.State -ne "Running") {
    Write-InstallError "服务启动失败（状态：$($Task.State)）"
  }
  $InstallChanged = $false
  Show-InstallResult
} catch {
  if ($InstallChanged) {
    Write-Warning "安装未完成，正在恢复上一版本"
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    if ($HadPreviousAgent) {
      Copy-Item -LiteralPath $PreviousAgent -Destination $AgentFile -Force
    } else {
      Remove-Item -LiteralPath $AgentFile -Force -ErrorAction SilentlyContinue
    }
    if ($HadPreviousConfig) {
      Copy-Item -LiteralPath $PreviousConfig -Destination $ConfigFile -Force
    } else {
      Remove-Item -LiteralPath $ConfigFile -Force -ErrorAction SilentlyContinue
    }
    if ($HadPreviousLauncher) {
      Copy-Item -LiteralPath $PreviousLauncher -Destination $LauncherFile -Force
    } else {
      Remove-Item -LiteralPath $LauncherFile -Force -ErrorAction SilentlyContinue
    }
    if ($null -ne $PreviousTaskXml) {
      Register-ScheduledTask -TaskName $TaskName -Xml $PreviousTaskXml -Force | Out-Null
      Start-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    }
  }
  throw
} finally {
  Remove-Item -LiteralPath $Temporary -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $PreviousAgent -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $PreviousConfig -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $PreviousLauncher -Force -ErrorAction SilentlyContinue
}
