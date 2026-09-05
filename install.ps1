[CmdletBinding(DefaultParameterSetName = "Menu")]
param(
  [Parameter(ParameterSetName = "Menu")][switch]$Menu,
  [Parameter(ParameterSetName = "Install", Mandatory = $true)][switch]$Install,
  [Parameter(ParameterSetName = "Uninstall", Mandatory = $true)][switch]$Uninstall,
  [Parameter(ParameterSetName = "Uninstall")][switch]$Purge,
  [Parameter(ParameterSetName = "Status", Mandatory = $true)][switch]$Status,
  [Parameter(ParameterSetName = "Restart", Mandatory = $true)][switch]$Restart
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

function Show-Menu {
  Write-Host ""
  Write-Host "NodeFlare 面板管理"
  Write-Host "  1. 安装 / 更新"
  Write-Host "  2. 查看服务状态"
  Write-Host "  3. 重启服务"
  Write-Host "  4. 卸载（保留配置和数据）"
  Write-Host "  0. 退出"
  while ($true) {
    switch ((Read-Host "请选择 [0]").Trim()) {
      "1" { return "Install" }
      "2" { return "Status" }
      "3" { return "Restart" }
      "4" {
        if ((Read-Host "确认卸载 NodeFlare 面板？[y/N]").Trim() -eq "y") { return "Uninstall" }
        return "Exit"
      }
      "0" { return "Exit" }
      "" { return "Exit" }
      default { Write-Host "请输入 0-4。" }
    }
  }
}

function Read-Port {
  while ($true) {
    $Value = (Read-Host "监听端口 [2206]").Trim()
    if (-not $Value) { return 2206 }
    if ($Value -match '^[0-9]{1,5}$') {
      $Port = [int]$Value
      if ($Port -ge 1 -and $Port -le 65535) { return $Port }
    }
    Write-Host "端口必须是 1-65535 之间的整数。"
  }
}

function Wait-Server {
  $Task = $null
  for ($Attempt = 0; $Attempt -lt 10; $Attempt++) {
    Start-Sleep -Milliseconds 500
    $Task = Get-ScheduledTask -TaskName $TaskName
    if ($Task.State -eq "Running") { break }
  }
  if ($Task.State -ne "Running") {
    Stop-Install "NodeFlare 服务启动失败（状态：$($Task.State)）"
  }
  for ($Attempt = 0; $Attempt -lt 10; $Attempt++) {
    Start-Sleep -Seconds 1
    $Task = Get-ScheduledTask -TaskName $TaskName
    if ($Task.State -ne "Running") {
      Stop-Install "NodeFlare 启动后退出，请检查 bind_addr 端口占用及数据库连接"
    }
  }
}

function Write-Config([string]$Username, [string]$Password, [string]$DatabaseUrl, [int]$Port) {
  $FrontendDir = (Join-Path $ShareDir "frontend").Replace('\', '/')
  $AdminDir = (Join-Path $ShareDir "admin").Replace('\', '/')
  $AgentDir = (Join-Path $ShareDir "agent").Replace('\', '/')
  $ThemePath = $ThemeDir.Replace('\', '/')
  $Lines = @(
    "database_url = `"$(Escape-Toml $DatabaseUrl)`""
    "bind_addr = `"127.0.0.1:$Port`""
    'trusted_proxies = ["127.0.0.1/32", "::1/128"]'
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

$Mode = $PSCmdlet.ParameterSetName
if ($Mode -eq "Menu") { $Mode = Show-Menu }
if ($Mode -eq "Exit") { exit 0 }

if ($Mode -eq "Status") {
  $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -eq $Task) {
    Write-Error "未检测到 NodeFlare 服务"
    exit 1
  }
  $Task
  exit 0
}

Assert-Administrator

if ($Mode -eq "Restart") {
  $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -eq $Task -or -not (Test-Path -LiteralPath $ServerFile) -or -not (Test-Path -LiteralPath $ConfigFile)) {
    Stop-Install "未检测到完整安装，请先选择安装 / 更新"
  }
  Stop-ScheduledTask -TaskName $TaskName
  Start-ScheduledTask -TaskName $TaskName
  Wait-Server
  Write-Step "NodeFlare 已重启"
  exit 0
}

if ($Mode -eq "Uninstall") {
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
$ReleaseApi = "https://api.github.com/repos/$Repository/releases/latest"
$TemporaryDir = Join-Path $env:TEMP "nodeflare-install-$PID"
$Archive = Join-Path $TemporaryDir $Asset
$PackageDir = Join-Path $TemporaryDir "package"
$PreviousInstall = Join-Path $TemporaryDir "previous-install"
$InstallChanged = $false
$HadPreviousInstall = $false
$PreviousTaskXml = $null

try {
  New-Item -ItemType Directory -Path $TemporaryDir -Force | Out-Null
  Write-Step "获取 latest Release（Windows x64）"
  $Release = Invoke-RestMethod -Uri $ReleaseApi -Headers @{ Accept = "application/vnd.github+json"; "User-Agent" = "nodeflare-installer" } -TimeoutSec 30
  $ReleaseAsset = $Release.assets | Where-Object { $_.name -eq $Asset } | Select-Object -First 1
  if ($Release.tag_name -notmatch '^v[0-9]+\.[0-9]+\.[0-9]+$' -or $null -eq $ReleaseAsset) {
    Stop-Install "GitHub 最新 Release 无效，或缺少 $Asset"
  }
  $DigestMatch = [regex]::Match([string]$ReleaseAsset.digest, '^sha256:([0-9a-fA-F]{64})$')
  if (-not $DigestMatch.Success) { Stop-Install "Release 缺少 $Asset 的 SHA-256 摘要" }
  $Expected = $DigestMatch.Groups[1].Value
  Write-Step "下载 NodeFlare $($Release.tag_name)（Windows x64）"
  Invoke-WebRequest -UseBasicParsing -Uri $ReleaseAsset.browser_download_url -OutFile $Archive -TimeoutSec 120
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
    (Join-Path $PackageDir "share\agent\agent.sh"),
    (Join-Path $PackageDir "LICENSE")
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
  if ($Version -ne $Release.tag_name.Substring(1)) {
    Stop-Install "Release $($Release.tag_name) 与服务端版本 $Version 不一致"
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
    $Port = Read-Port
    do {
      $DatabaseUrl = (Read-Host "数据库 URL [sqlite://nodeflare.db]").Trim()
      if (-not $DatabaseUrl) { $DatabaseUrl = "sqlite://nodeflare.db" }
    } while ($DatabaseUrl -notmatch '^(sqlite|postgres|postgresql)://')
  }

  Write-Step "安装 NodeFlare $Version"
  $HadPreviousInstall = Test-Path -LiteralPath $InstallDir -PathType Container
  if ($HadPreviousInstall) {
    Copy-Item -LiteralPath $InstallDir -Destination $PreviousInstall -Recurse -Force
  }
  $ExistingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -ne $ExistingTask) {
    $PreviousTaskXml = Export-ScheduledTask -TaskName $TaskName
  }
  $InstallChanged = $true
  Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Path $InstallDir, $DataDir, $ThemeDir -Force | Out-Null
  Remove-Item -LiteralPath $ShareDir -Recurse -Force -ErrorAction SilentlyContinue
  Copy-Item -LiteralPath (Join-Path $PackageDir "share") -Destination $ShareDir -Recurse -Force
  Copy-Item -LiteralPath $PackageServer -Destination $ServerFile -Force
  if ($NewConfig) {
    Write-Config $Username $Password $DatabaseUrl $Port
  }
  & icacls.exe $DataDir /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
  if ($LASTEXITCODE -ne 0) { Stop-Install "无法限制配置和数据目录权限" }

  $Action = New-ScheduledTaskAction -Execute $ServerFile -Argument "--config `"$ConfigFile`"" -WorkingDirectory $DataDir
  $Trigger = New-ScheduledTaskTrigger -AtStartup
  $Settings = New-ScheduledTaskSettingsSet -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Days 3650)
  $Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
  Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
  Start-ScheduledTask -TaskName $TaskName

  Wait-Server

  Write-Host ""
  Write-Host "NodeFlare 安装完成"
  Write-Host "  版本：$Version"
  Write-Host "  配置和数据：$DataDir"
  Write-Host "  服务：Windows 计划任务 $TaskName"
  if ($NewConfig) {
    Write-Host "  本机访问：http://127.0.0.1:$Port/admin/login"
  } else {
    Write-Host "  监听地址：沿用 $ConfigFile 中的 bind_addr"
  }
  $InstallChanged = $false
} catch {
  if ($InstallChanged) {
    Write-Warning "安装未完成，正在恢复上一版本"
    Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction SilentlyContinue
    if ($HadPreviousInstall) {
      Copy-Item -LiteralPath $PreviousInstall -Destination $InstallDir -Recurse -Force
    }
    if ($null -ne $PreviousTaskXml) {
      Register-ScheduledTask -TaskName $TaskName -Xml $PreviousTaskXml -Force | Out-Null
      Start-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    }
  }
  throw
} finally {
  Remove-Item -LiteralPath $TemporaryDir -Recurse -Force -ErrorAction SilentlyContinue
}
