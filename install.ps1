[CmdletBinding(DefaultParameterSetName = "Install")]
param(
  [Parameter(ParameterSetName = "Uninstall", Mandatory = $true)][switch]$Uninstall,
  [Parameter(ParameterSetName = "Uninstall")][switch]$Purge,
  [Parameter(ParameterSetName = "Status", Mandatory = $true)][switch]$Status
)

$ErrorActionPreference = "Stop"
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$Repository = "imengying/NodeFlare"
$TaskName = "nodeflare"
$InstallDir = Join-Path $env:ProgramFiles "NodeFlare"
$ServerFile = Join-Path $InstallDir "nodeflare.exe"
$ShareDir = Join-Path $InstallDir "share"
$DataDir = Join-Path $env:ProgramData "NodeFlare\Server"
$ConfigFile = Join-Path $DataDir "config.toml"
$ThemeDir = Join-Path $DataDir "themes"

function Write-Step([string]$Message) {
  Write-Host "[NodeFlare] $Message"
}

function Stop-Install([string]$Message) {
  throw "[NodeFlare] 错误：$Message"
}

function Assert-Administrator {
  $Identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $Principal = [Security.Principal.WindowsPrincipal]$Identity
  if (-not $Principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Stop-Install "请使用管理员身份运行 PowerShell"
  }
}

function Read-Password([string]$Prompt) {
  $Secure = Read-Host $Prompt -AsSecureString
  $Pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($Secure)
  try {
    [Runtime.InteropServices.Marshal]::PtrToStringBSTR($Pointer)
  } finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($Pointer)
  }
}

function Escape-Toml([string]$Value) {
  $Value.Replace('\', '\\').Replace('"', '\"')
}

function Write-Config([string]$Username, [string]$Password, [string]$DatabaseUrl) {
  $FrontendDir = (Join-Path $ShareDir "frontend").Replace('\', '/')
  $AdminDir = (Join-Path $ShareDir "admin").Replace('\', '/')
  $AgentDir = (Join-Path $ShareDir "agent").Replace('\', '/')
  $ThemePath = $ThemeDir.Replace('\', '/')
  $Lines = @(
    "database_url = `"$(Escape-Toml $DatabaseUrl)`""
    'bind_addr = "127.0.0.1:8080"'
    "admin_username = `"$(Escape-Toml $Username)`""
    "admin_password = `"$(Escape-Toml $Password)`""
    'turnstile_site_key = ""'
    'turnstile_secret_key = ""'
    "frontend_dir = `"$FrontendDir`""
    "admin_frontend_dir = `"$AdminDir`""
    "agent_dir = `"$AgentDir`""
    "theme_dir = `"$ThemePath`""
    'session_ttl_hours = 168'
  )
  [IO.File]::WriteAllLines($ConfigFile, $Lines, [Text.UTF8Encoding]::new($false))
}

if ($Status) {
  $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -eq $Task) {
    Write-Error "未检测到 NodeFlare 服务"
    exit 1
  }
  $Task
  exit 0
}

Assert-Administrator

if ($Uninstall) {
  Write-Step "停止并移除 NodeFlare 面板服务"
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $ServerFile -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $ShareDir -Recurse -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $InstallDir -Force -ErrorAction SilentlyContinue
  if ($Purge) {
    Remove-Item -LiteralPath $DataDir -Recurse -Force -ErrorAction SilentlyContinue
    Write-Step "已删除配置和全部服务端持久数据"
  } else {
    Write-Step "已保留配置和服务端持久数据：$DataDir"
  }
  Write-Step "卸载完成"
  exit 0
}

$NativeArchitecture = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($NativeArchitecture -ne "AMD64") {
  Stop-Install "仅支持 Windows x64"
}

$Asset = "nodeflare-server-windows-x64.zip"
$ReleaseBase = "https://github.com/$Repository/releases/latest/download"
$TemporaryDir = Join-Path $env:TEMP "nodeflare-install-$PID"
$Archive = Join-Path $TemporaryDir $Asset
$Checksum = "$Archive.sha256"
$PackageDir = Join-Path $TemporaryDir "package"

try {
  New-Item -ItemType Directory -Path $TemporaryDir -Force | Out-Null
  Write-Step "下载 latest Release（Windows x64）"
  Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/$Asset" -OutFile $Archive -TimeoutSec 120
  Invoke-WebRequest -UseBasicParsing -Uri "$ReleaseBase/$Asset.sha256" -OutFile $Checksum -TimeoutSec 30
  $Expected = ((Get-Content -LiteralPath $Checksum -Raw).Trim() -split '\s+')[0]
  if ($Expected -notmatch '^[0-9a-fA-F]{64}$') {
    Stop-Install "Release 校验文件无效"
  }
  $Actual = (Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash
  if ($Actual -ne $Expected) {
    Stop-Install "Release SHA-256 校验失败"
  }
  Expand-Archive -LiteralPath $Archive -DestinationPath $PackageDir -Force
  $PackageServer = Join-Path $PackageDir "nodeflare.exe"
  foreach ($Required in @(
    $PackageServer,
    (Join-Path $PackageDir "share\frontend\index.html"),
    (Join-Path $PackageDir "share\admin\admin.html"),
    (Join-Path $PackageDir "share\agent\agent.sh")
  )) {
    if (-not (Test-Path -LiteralPath $Required -PathType Leaf)) {
      Stop-Install "Release 文件不完整"
    }
  }
  Unblock-File -LiteralPath $PackageServer
  $VersionOutput = (& $PackageServer --version | Out-String).Trim()
  if ($LASTEXITCODE -ne 0) {
    Stop-Install "服务端文件无法运行"
  }
  $Version = ($VersionOutput -split '\s+')[-1]
  if ($Version -notmatch '^\d+\.\d+\.\d+$') {
    Stop-Install "服务端返回了无效版本号"
  }

  $NewConfig = -not (Test-Path -LiteralPath $ConfigFile)
  if ($NewConfig) {
    do {
      $Username = (Read-Host "管理员用户名 [admin]").Trim()
      if (-not $Username) { $Username = "admin" }
    } while ($Username -notmatch '^[A-Za-z0-9_.-]{1,64}$')
    do {
      $Password = Read-Password "管理员密码（8-128 个字符）"
      $Confirmed = Read-Password "再次输入密码"
    } while ($Password.Length -lt 8 -or $Password.Length -gt 128 -or $Password -ne $Confirmed)
    do {
      $DatabaseUrl = (Read-Host "数据库 URL [sqlite://nodeflare.db]").Trim()
      if (-not $DatabaseUrl) { $DatabaseUrl = "sqlite://nodeflare.db" }
    } while ($DatabaseUrl -notmatch '^(sqlite|postgres|postgresql)://')
  }

  Write-Step "安装 NodeFlare $Version"
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $InstallDir, $DataDir, $ThemeDir -Force | Out-Null
  Remove-Item -LiteralPath $ShareDir -Recurse -Force -ErrorAction SilentlyContinue
  Copy-Item -LiteralPath (Join-Path $PackageDir "share") -Destination $ShareDir -Recurse -Force
  Copy-Item -LiteralPath $PackageServer -Destination $ServerFile -Force
  if ($NewConfig) {
    Write-Config $Username $Password $DatabaseUrl
  }
  & icacls.exe $DataDir /inheritance:r /grant:r 'SYSTEM:(OI)(CI)F' 'Administrators:(OI)(CI)F' | Out-Null

  $Action = New-ScheduledTaskAction -Execute $ServerFile -Argument "--config `"$ConfigFile`"" -WorkingDirectory $DataDir
  $Trigger = New-ScheduledTaskTrigger -AtStartup
  $Settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Days 3650)
  $Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
  Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
  Start-ScheduledTask -TaskName $TaskName

  $Task = $null
  for ($Attempt = 0; $Attempt -lt 10; $Attempt++) {
    Start-Sleep -Milliseconds 500
    $Task = Get-ScheduledTask -TaskName $TaskName
    if ($Task.State -eq "Running") { break }
  }
  if ($Task.State -ne "Running") {
    Stop-Install "NodeFlare 服务启动失败（状态：$($Task.State)）"
  }

  Write-Host ""
  Write-Host "NodeFlare 安装完成"
  Write-Host "  版本：$Version"
  Write-Host "  配置和数据：$DataDir"
  Write-Host "  服务：Windows 计划任务 $TaskName"
  Write-Host "  默认访问：http://127.0.0.1:8080"
} finally {
  Remove-Item -LiteralPath $TemporaryDir -Recurse -Force -ErrorAction SilentlyContinue
}
