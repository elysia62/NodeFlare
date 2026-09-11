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
$Repository = "elysia62/NodeFlare"
$TaskName = "nodeflare"
$InstallDir = Join-Path $env:ProgramFiles "NodeFlare"
$ServerFile = Join-Path $InstallDir "nodeflare.exe"
$ShareDir = Join-Path $InstallDir "share"
$DataDir = Join-Path $env:ProgramData "NodeFlare\Server"
$ConfigFile = Join-Path $DataDir "config.toml"
$ThemeDir = Join-Path $DataDir "themes"

function Write-Step([string]$Message) {
  Write-Host $Message
}

function Stop-Install([string]$Message) {
  throw "错误：$Message"
}

function Show-InstallResult {
  Write-Host ""
  if (-not $NewConfig) {
    Write-Host "更新完成（v$Version）"
    return
  }
  Write-Host "安装完成（v$Version）"
  Write-Host "  配置和数据：$DataDir"
  Write-Host "  服务：Windows 计划任务 $TaskName"
  Write-Host "  本机访问：http://127.0.0.1:$Port/admin/login"
  Write-Host "  下一步：登录管理后台创建节点，并按弹窗命令安装 Agent"
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
  $CurrentVersion = $null
  if (Test-Path -LiteralPath $ServerFile -PathType Leaf) {
    try { $VersionOutput = (& $ServerFile --version 2>$null | Out-String).Trim() } catch { $VersionOutput = "" }
    if ($VersionOutput -match '(\d+\.\d+\.\d+)\s*$') { $CurrentVersion = $Matches[1] }
  }
  $InstallAction = if ($CurrentVersion) { "更新（当前 v$CurrentVersion）" } else { "安装" }
  Write-Host ""
  Write-Host "NodeFlare 面板管理"
  Write-Host "  1. $InstallAction"
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

function Stop-Server {
  $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -ne $Task) { Stop-ScheduledTask -TaskName $TaskName -ErrorAction Stop }
  for ($Attempt = 0; $Attempt -lt 30; $Attempt++) {
    $Task = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    $Running = @(Get-CimInstance Win32_Process -Filter "Name = 'nodeflare.exe'" -ErrorAction Stop |
      Where-Object { $_.ExecutablePath -eq $ServerFile })
    if (($null -eq $Task -or $Task.State -ne "Running") -and $Running.Count -eq 0) { return }
    Start-Sleep -Milliseconds 500
  }
  Stop-Install "旧面板进程未退出，已停止操作，未替换程序"
}

function Restore-ServerFiles {
  if (Test-Path -LiteralPath $ServerFile) { Remove-Item -LiteralPath $ServerFile -Force -ErrorAction Stop }
  if (Test-Path -LiteralPath $ShareDir) { Remove-Item -LiteralPath $ShareDir -Recurse -Force -ErrorAction Stop }
  if ($HadPreviousInstall) {
    Copy-Item -LiteralPath (Join-Path $PreviousInstall "nodeflare.exe") -Destination $ServerFile -Force
  }
  if ($HadPreviousShare) {
    Copy-Item -LiteralPath (Join-Path $PreviousInstall "share") -Destination $ShareDir -Recurse -Force
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
    Stop-Install "服务启动失败（状态：$($Task.State)）"
  }
  for ($Attempt = 0; $Attempt -lt 10; $Attempt++) {
    Start-Sleep -Seconds 1
    $Task = Get-ScheduledTask -TaskName $TaskName
    if ($Task.State -ne "Running") {
      Stop-Install "服务启动后退出，请检查 bind_addr 端口占用及数据库连接"
    }
  }
}

function Write-Config([string]$Username, [string]$Password, [string]$DatabaseUrl, [int]$Port) {
  $FrontendDir = (Join-Path $ShareDir "frontend").Replace('\', '/')
  $AdminDir = (Join-Path $ShareDir "admin").Replace('\', '/')
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
    Write-Error "未检测到面板服务"
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
  Stop-Server
  Start-ScheduledTask -TaskName $TaskName
  Wait-Server
  Write-Step "服务已重启"
  exit 0
}

if ($Mode -eq "Uninstall") {
  if ($Purge -and -not [Console]::IsInputRedirected) {
    if ((Read-Host "即将删除全部配置和数据，确认继续？[y/N]").Trim() -ne "y") {
      Stop-Install "已取消卸载"
    }
  }
  Write-Step "停止并移除面板服务"
  Stop-Server
  Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $ServerFile -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $ShareDir -Recurse -Force -ErrorAction SilentlyContinue
  try { [IO.Directory]::Delete($InstallDir, $false) } catch [IO.IOException] { }
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
$HadPreviousShare = $false
$PreviousTaskXml = $null
$KeepBackup = $false

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
  Write-Step "下载 $($Release.tag_name)（Windows x64）"
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
      if ($Username -notmatch '^[A-Za-z0-9_.-]{1,64}$') { Write-Host "用户名只能包含字母、数字、点、下划线和连字符（1-64 个字符）。" }
    } while ($Username -notmatch '^[A-Za-z0-9_.-]{1,64}$')
    while ($true) {
      $Password = Read-Password "管理员密码（8-128 个字符）"
      if ($Password.Length -lt 8 -or $Password.Length -gt 128) {
        Write-Host "密码长度必须在 8-128 个字符之间。"
        continue
      }
      $Confirmed = Read-Password "再次输入密码"
      if ($Password -ceq $Confirmed) { break }
      Write-Host "两次输入的密码不一致。"
    }
    $Port = Read-Port
    do {
      $DatabaseUrl = (Read-Host "数据库 URL [sqlite://nodeflare.db]").Trim()
      if (-not $DatabaseUrl) { $DatabaseUrl = "sqlite://nodeflare.db" }
    } while ($DatabaseUrl -notmatch '^(sqlite|postgres|postgresql)://')
  }

  if ($NewConfig) {
    Write-Step "正在安装 v$Version"
  } else {
    Write-Step "正在更新至 v$Version"
  }
  $HadPreviousInstall = Test-Path -LiteralPath $ServerFile -PathType Leaf
  $HadPreviousShare = Test-Path -LiteralPath $ShareDir -PathType Container
  New-Item -ItemType Directory -Path $PreviousInstall -Force | Out-Null
  if ($HadPreviousInstall) {
    Copy-Item -LiteralPath $ServerFile -Destination (Join-Path $PreviousInstall "nodeflare.exe") -Force
  }
  if ($HadPreviousShare) {
    Copy-Item -LiteralPath $ShareDir -Destination (Join-Path $PreviousInstall "share") -Recurse -Force
  }
  $ExistingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
  if ($null -ne $ExistingTask) {
    $PreviousTaskXml = Export-ScheduledTask -TaskName $TaskName
  }
  Stop-Server
  $InstallChanged = $true
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
  $Settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -RestartCount 999 -RestartInterval (New-TimeSpan -Minutes 1) -ExecutionTimeLimit (New-TimeSpan -Days 3650)
  $Principal = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
  Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
  Write-Step "正在启动服务"
  Start-ScheduledTask -TaskName $TaskName

  Wait-Server

  $InstallChanged = $false
  Show-InstallResult
} catch {
  $InstallError = $_
  if ($InstallChanged) {
    if ($HadPreviousInstall) { Write-Warning "安装未完成，正在恢复上一版本" }
    else { Write-Warning "安装未完成，正在回滚本次更改" }
    try {
      Stop-Server
      Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
      Restore-ServerFiles
      if ($null -ne $PreviousTaskXml) {
        Register-ScheduledTask -TaskName $TaskName -Xml $PreviousTaskXml -Force | Out-Null
        Start-ScheduledTask -TaskName $TaskName -ErrorAction Stop
      }
    } catch {
      $KeepBackup = $true
      Write-Warning "自动回滚未完成，备份已保留在 ${PreviousInstall}：$_"
    }
  }
  throw $InstallError
} finally {
  if (-not $KeepBackup) {
    Remove-Item -LiteralPath $TemporaryDir -Recurse -Force -ErrorAction SilentlyContinue
  }
}
