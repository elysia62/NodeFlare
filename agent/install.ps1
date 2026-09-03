[CmdletBinding(DefaultParameterSetName = "Install")]
param(
  [Parameter(ParameterSetName = "Install", Mandatory = $true)][Alias("t")][string]$Token,
  [Parameter(ParameterSetName = "Install", Mandatory = $true)][Alias("e")][string]$Endpoint,
  [Parameter(ParameterSetName = "Install")][Alias("i")][ValidateRange(15, 3600)][int]$Interval = 60,
  [Parameter(ParameterSetName = "Install")][Alias("m")][string]$Mirror = "",
  [Parameter(ParameterSetName = "Uninstall", Mandatory = $true)][switch]$Uninstall,
  [Parameter(ParameterSetName = "Status", Mandatory = $true)][switch]$Status
)

$ErrorActionPreference = "Stop"
$TaskName = "nodeflare-agent"
$InstallDir = Join-Path $env:ProgramFiles "NodeFlare"
$DataDir = Join-Path $InstallDir "data"
$StateDir = Join-Path $DataDir "agent"
$AgentFile = Join-Path $InstallDir "agent.exe"

function Write-Step([string]$Message) {
  Write-Host "[NodeFlare] $Message"
}

function Write-InstallError([string]$Message) {
  throw "[NodeFlare] 错误：$Message"
}

function Assert-Safe([string]$Name, [string]$Value) {
  if ([string]::IsNullOrWhiteSpace($Value) -or $Value -notmatch '^[A-Za-z0-9_./:@-]+$') {
    Write-InstallError "$Name 格式无效"
  }
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
  Write-Step "正在停止并移除 NodeFlare Agent"
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $AgentFile -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $StateDir -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $DataDir -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $InstallDir -Force -ErrorAction SilentlyContinue
  Write-Host "NodeFlare Agent 已卸载"
  exit 0
}

if ($Status) {
  $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -eq $Task) {
    Write-Error "未检测到 NodeFlare Agent 服务"
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
& icacls.exe $InstallDir /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' | Out-Null
& icacls.exe $StateDir /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' | Out-Null
$Temporary = "$AgentFile.$PID.download.exe"
$ReleaseApi = "https://api.github.com/repos/imengying/NodeFlare/releases/latest"
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
  $DownloadUrl = "https://github.com/imengying/NodeFlare/releases/download/$($Release.tag_name)/$Artifact"
  if ($Mirror) {
    $DownloadUrl = "$Mirror/$DownloadUrl"
    Write-Step "正在通过下载加速前缀拉取 NodeFlare Agent $($Release.tag_name)"
  } else {
    Write-Step "正在下载 NodeFlare Agent $($Release.tag_name)"
  }
  Invoke-WebRequest -Uri $DownloadUrl -OutFile $Temporary -TimeoutSec 120
  $ActualChecksum = (Get-FileHash -LiteralPath $Temporary -Algorithm SHA256).Hash
  if ($ActualChecksum -ne $ExpectedChecksum) {
    Write-InstallError "Agent SHA-256 校验失败，已停止安装"
  }
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
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Move-Item -LiteralPath $Temporary -Destination $AgentFile -Force
} finally {
  Remove-Item -LiteralPath $Temporary -Force -ErrorAction SilentlyContinue
}

$TaskArguments = "-e `"$Endpoint`" -t `"$Token`" -i $Interval"
Write-Step "正在注册并启动 Windows 计划任务"
$TaskAction = New-ScheduledTaskAction -Execute $AgentFile -Argument $TaskArguments
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
  Write-InstallError "NodeFlare 服务启动失败（状态：$($Task.State)）"
}
Write-Host ""
Write-Host "NodeFlare Agent 安装完成"
Write-Host "  版本：$InstalledVersion"
Write-Host "  服务：$TaskName（Windows 计划任务）"
Write-Host "  查看状态：Get-ScheduledTask -TaskName '$TaskName'"
