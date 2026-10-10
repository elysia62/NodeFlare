import { describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";

const installer = readFileSync(new URL("../../install.sh", import.meta.url), "utf8");
const windowsInstaller = readFileSync(new URL("../../install.ps1", import.meta.url), "utf8");
const agentInstaller = readFileSync(new URL("../../agent/agent.sh", import.meta.url), "utf8");

function shellFunctions(...names: string[]) {
  return names.map((name) => {
    const match = installer.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
    if (!match) throw new Error(`Missing installer function: ${name}`);
    return match[0];
  }).join("\n");
}

function agentShellFunctions(...names: string[]) {
  return names.map((name) => {
    const match = agentInstaller.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
    if (!match) throw new Error(`Missing agent installer function: ${name}`);
    return match[0];
  }).join("\n");
}

const startFunction = shellFunctions("check_server_process", "wait_server_ready", "start_server");

describe("server uninstall ownership", () => {
  for (const [platform, purge] of [["linux", true], ["linux", false], ["macos", true], ["freebsd", true]] as const) {
    test(`${platform} purge=${purge} only removes server-owned files`, () => {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-uninstall-test-"));
      const config = join(directory, "config");
      try {
        mkdirSync(join(config, "agent"), { recursive: true });
        mkdirSync(join(directory, "install", "share"), { recursive: true });
        writeFileSync(join(config, "agent", "remote-tasks.json"), "agent-state");
        writeFileSync(join(config, "config.toml"), "server-config");
        writeFileSync(join(config, ".server-state"), "server-state");
        writeFileSync(join(directory, "install", "nodeflare"), "server-binary");
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${shellFunctions("log", "uninstall_server")}
          config_dir=$TEST_DIRECTORY/config
          install_dir=$TEST_DIRECTORY/install
          share_dir=$install_dir/share
          server_binary=$install_dir/nodeflare
          systemd_file=$TEST_DIRECTORY/server.service
          openrc_file=$TEST_DIRECTORY/server.openrc
          launchd_file=$TEST_DIRECTORY/server.plist
          freebsd_rc_file=$TEST_DIRECTORY/server.rc
          platform=$TEST_PLATFORM
          init_system=systemd
          stop_server() { :; }
          systemctl() { :; }
          uninstall_server "$TEST_PURGE"
        `], { env: { ...process.env, TEST_DIRECTORY: directory, TEST_PLATFORM: platform, TEST_PURGE: String(purge) }, encoding: "utf8" });
        expect(result.status).toBe(0);
        expect(existsSync(join(directory, "install"))).toBe(false);
        expect(existsSync(join(config, "config.toml"))).toBe(!purge);
        expect(existsSync(join(config, ".server-state"))).toBe(!purge);
        if (platform === "linux") {
          expect(readFileSync(join(config, "agent", "remote-tasks.json"), "utf8")).toBe("agent-state");
        } else {
          expect(existsSync(config)).toBe(false);
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

describe("Windows uninstall ownership", () => {
  const powershell = Bun.which("pwsh");
  for (const kind of ["server", "agent"] as const) {
    const source = readFileSync(new URL(kind === "server" ? "../../install.ps1" : "../../agent/install.ps1", import.meta.url), "utf8");
    const branch = kind === "server"
      ? source.match(/^if \(\$Mode -eq "Uninstall"\) \{[\s\S]*?^\}/m)?.[0]
      : source.match(/^if \(\$Uninstall\) \{[\s\S]*?^\}/m)?.[0];
    if (!branch) throw new Error(`Missing Windows ${kind} uninstall branch`);

    test.skipIf(!powershell)(`${kind} keeps the other service's shared files and data`, () => {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-windows-uninstall-"));
      const files = {
        "install/nodeflare.exe": "server-binary",
        "install/share/frontend/index.html": "server-frontend",
        "install/agent.exe": "agent-binary",
        "install/run-agent.ps1": "agent-launcher",
        "data/Server/config.toml": "server-data",
        "data/Agent/config.json": "agent-data",
      };
      try {
        for (const [path, content] of Object.entries(files)) {
          mkdirSync(dirname(join(directory, path)), { recursive: true });
          writeFileSync(join(directory, path), content);
        }
        const result = spawnSync(powershell!, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", `
          $ErrorActionPreference = "Stop"
          function Write-Step { }
          function Stop-ScheduledTask { }
          function Stop-Server { }
          function Stop-Agent { }
          function Unregister-ScheduledTask { }
          $Mode = "Uninstall"
          $Uninstall = $true
          $Purge = $false
          $TaskName = "unused"
          $InstallDir = Join-Path $env:TEST_DIRECTORY "install"
          $ShareDir = Join-Path $InstallDir "share"
          $ServerFile = Join-Path $InstallDir "nodeflare.exe"
          $AgentFile = Join-Path $InstallDir "agent.exe"
          $LauncherFile = Join-Path $InstallDir "run-agent.ps1"
          $DataDir = Join-Path $env:TEST_DIRECTORY "data"
          $StateDir = Join-Path $DataDir "Agent"
          if ($env:TEST_KIND -eq "server") { $DataDir = Join-Path $DataDir "Server" }
          ${branch}
        `], { env: { ...process.env, TEST_DIRECTORY: directory, TEST_KIND: kind }, encoding: "utf8", timeout: 10_000 });
        expect(result.stderr).toBe("");
        expect(result.status).toBe(0);
        const peerPaths = kind === "server"
          ? ["install/agent.exe", "install/run-agent.ps1", "data/Agent/config.json"]
          : ["install/nodeflare.exe", "install/share/frontend/index.html", "data/Server/config.toml"];
        for (const path of peerPaths) {
          expect(readFileSync(join(directory, path), "utf8")).toBe(files[path as keyof typeof files]);
        }
        expect(existsSync(join(directory, kind === "server" ? "install/nodeflare.exe" : "install/agent.exe"))).toBe(false);
        if (kind === "agent") {
          expect(existsSync(join(directory, "install/run-agent.ps1"))).toBe(false);
          expect(existsSync(join(directory, "data/Agent"))).toBe(false);
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    }, 30_000);
  }
});

function start(scenario: string, init = "systemd") {
  const directory = mkdtempSync(join(tmpdir(), "nodeflare-startup-"));
  try {
    // Dash rejects hyphenated function names, so mock the external commands.
    writeFileSync(join(directory, "rc-service"),
      '#!/bin/sh\n[ "$2" != status ] || [ "$SCENARIO" != exits ] || [ "$tick" -lt 1 ]\n', { mode: 0o755 });
    writeFileSync(join(directory, "rc-update"),
      '#!/bin/sh\n[ "$SCENARIO" != command-failure ]\n', { mode: 0o755 });
    return spawnSync("sh", ["-c", `
      set -eu
      init_system=$TEST_INIT
      launchd_file=/unused/nodeflare.plist
      server_binary=probe
      config_file='/unused/server config.toml'
      if [ "$SCENARIO" = staged-probe ]; then readiness_binary=probe; server_binary=old_probe; fi
      tick=0
      export tick
      log() { :; }
      date() { printf '%s\n' "$tick"; }
      sleep() { tick=$((tick + 1)); }
      probe() {
        [ "$#" -eq 3 ] && [ "$1" = --config ] && [ "$2" = "$config_file" ] && [ "$3" = --check-ready ] || return 99
        case "$SCENARIO" in
          never-ready) return 1 ;;
          delayed-ready) [ "$tick" -ge 3 ] ;;
          flaky-ready) [ "$tick" -ne 1 ] ;;
          exits-during-probe) tick=1; return 0 ;;
          *) return 0 ;;
        esac
      }
      old_probe() { return 99; }
      systemctl() {
        case "$1" in
          daemon-reload) [ "$SCENARIO" != command-failure ] ;;
          enable|restart) return 0 ;;
          is-active)
            case "$SCENARIO" in exits|exits-during-probe) [ "$tick" -lt 1 ] ;; *) return 0 ;; esac ;;
          show)
            [ "$SCENARIO" != query-failure ] || return 1
            if [ "$SCENARIO" = restarts ] && [ "$tick" -ge 1 ]; then
              printf '456\n'
            else
              printf '123\n'
            fi ;;
          *) return 99 ;;
        esac
      }
      launchctl() {
        case "$1" in
          bootstrap) [ "$SCENARIO" != command-failure ] ;;
          print)
            [ "$SCENARIO" != query-failure ] || return 1
            if [ "$SCENARIO" = waiting ] || { [ "$SCENARIO" = delayed-process ] && [ "$tick" -lt 2 ]; } \
              || { { [ "$SCENARIO" = exits ] || [ "$SCENARIO" = exits-during-probe ]; } && [ "$tick" -ge 1 ]; }; then
              printf 'state = waiting\nlast exit code = 1\n'
            elif [ "$SCENARIO" = restarts ] && [ "$tick" -ge 1 ]; then
              printf 'state = running\npid = 456\n'
            else
              printf 'state = running\npid = 123\n'
            fi ;;
          *) return 99 ;;
        esac
      }
      sysrc() { [ "$SCENARIO" != command-failure ]; }
      service() {
        [ "$2" != status ] || [ "$SCENARIO" != exits ] || [ "$tick" -lt 1 ]
      }
      ${startFunction}
      if start_server; then printf 'ready:%s\n' "$tick"; else exit 1; fi
    `], {
      env: {
        ...process.env,
        PATH: `${directory}${delimiter}${process.env.PATH ?? ""}`,
        SCENARIO: scenario,
        TEST_INIT: init,
      },
      encoding: "utf8",
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("installer startup verification", () => {
  test("uses the staged probe when checking a restored older binary", () => {
    const result = start("staged-probe");
    expect(result.status).toBe(0);
    expect(result.stdout).toBe("ready:1\n");
  });
  for (const init of ["systemd", "launchd", "openrc", "freebsd"]) {
    for (const [scenario, ticks] of [["healthy", 1], ["delayed-ready", 4], ["flaky-ready", 3]] as const) {
      test(`${init} ${scenario} returns after consecutive HTTP readiness checks`, () => {
        const result = start(scenario, init);
        expect(result.status).toBe(0);
        expect(result.stdout).toBe(`ready:${ticks}\n`);
      });
    }
    test(`${init} bounds waiting when a live process never becomes HTTP ready`, () => {
      const result = start("never-ready", init);
      expect(result.status).toBe(1);
      expect(result.stderr).toContain("就绪超时（30 秒）");
    });
    for (const scenario of ["exits", "command-failure"]) {
      test(`${init} rejects ${scenario}`, () => {
        expect(start(scenario, init).status).toBe(1);
      });
    }
  }
  for (const scenario of ["restarts", "query-failure", "exits-during-probe"]) {
    test(`rejects ${scenario} even when called in an if condition`, () => {
      expect(start(scenario).status).toBe(1);
    });
  }
});

describe("launchd installer startup verification", () => {
  test("allows initial scheduling before checking a stable PID", () => {
    const result = start("delayed-process", "launchd");
    expect(result.status).toBe(0);
    expect(result.stdout).toBe("ready:3\n");
  });
  for (const scenario of ["restarts", "waiting", "query-failure", "exits-during-probe"]) {
    test(`rejects ${scenario}`, () => {
      expect(start(scenario, "launchd").status).toBe(1);
    });
  }
});

describe("Windows installer startup verification", () => {
  const powershell = Bun.which("pwsh");
  const functions = ["Get-ServerProcessId", "Test-ServerReady", "Wait-Server"].map((name) => {
    const match = windowsInstaller.match(new RegExp(`^function ${name} \\{[\\s\\S]*?^\\}`, "m"));
    if (!match) throw new Error(`Missing Windows installer function: ${name}`);
    return match[0];
  }).join("\n").replace("[Diagnostics.Stopwatch]::StartNew()", "([pscustomobject]@{ Elapsed = [pscustomobject]@{ TotalSeconds = 0 } })");

  for (const [scenario, ticks] of [["healthy", 1], ["delayed-ready", 4], ["delayed-process", 3], ["flaky-ready", 3],
    ["never-ready", null], ["exits", null], ["restarts", null], ["exits-during-probe", null], ["multiple-processes", null]] as const) {
    test.skipIf(!powershell)(`${scenario} preserves readiness and process safety`, () => {
      const script = `
        $ErrorActionPreference = "Stop"
        $script:Tick = 0
        $TaskName = "nodeflare"
        $ServerFile = "C:\\NodeFlare\\nodeflare.exe"
        $ReadinessFile = "Invoke-ReadinessProbe"
        $ConfigFile = "C:\\NodeFlare Data\\config.toml"
        function Write-Step { }
        function Stop-Install([string]$Message) { throw $Message }
        function Start-Sleep([int]$Seconds) { $script:Tick += $Seconds; $Timer.Elapsed.TotalSeconds = $script:Tick }
        function Get-ScheduledTask {
          $State = if (($env:SCENARIO -eq "delayed-process" -and $script:Tick -lt 2) -or
            ($env:SCENARIO -eq "exits" -and $script:Tick -ge 1)) { "Ready" } else { "Running" }
          [pscustomobject]@{ State = $State }
        }
        function Get-CimInstance {
          if (($env:SCENARIO -eq "delayed-process" -and $script:Tick -lt 2) -or
            (($env:SCENARIO -eq "exits" -or $env:SCENARIO -eq "exits-during-probe") -and $script:Tick -ge 1)) { return }
          $ProcessId = if ($env:SCENARIO -eq "restarts" -and $script:Tick -ge 1) { 456 } else { 123 }
          [pscustomobject]@{ ProcessId = $ProcessId; ExecutablePath = $ServerFile }
          if ($env:SCENARIO -eq "multiple-processes") { [pscustomobject]@{ ProcessId = 789; ExecutablePath = $ServerFile } }
        }
        function Invoke-ReadinessProbe {
          if ($args.Count -ne 3 -or $args[0] -ne "--config" -or $args[1] -ne $ConfigFile -or $args[2] -ne "--check-ready") {
            throw "Invalid readiness arguments"
          }
          $global:LASTEXITCODE = if ($env:SCENARIO -eq "never-ready" -or
            ($env:SCENARIO -eq "delayed-ready" -and $script:Tick -lt 3) -or
            ($env:SCENARIO -eq "flaky-ready" -and $script:Tick -eq 1)) { 1 } else { 0 }
          if ($env:SCENARIO -eq "exits-during-probe") { $script:Tick = 1 }
        }
        ${functions}
        try { Wait-Server; Write-Output "ready:$script:Tick" } catch { Write-Output $_; exit 1 }
      `;
      const result = spawnSync(powershell!, ["-NoProfile", "-NonInteractive", "-Command", script], {
        env: { ...process.env, SCENARIO: scenario }, encoding: "utf8", timeout: 30_000,
      });
      expect(result.status).toBe(ticks === null ? 1 : 0);
      if (ticks !== null) expect(result.stdout.trim()).toBe(`ready:${ticks}`);
      if (scenario === "never-ready") expect(result.stdout).toContain("就绪超时（30 秒）");
    }, 30_000);
  }
});

describe("installer port selection", () => {
  test("writes the default or chosen port to a valid TOML configuration", () => {
    const directory = mkdtempSync(join(tmpdir(), "nodeflare-install-test-"));
    try {
      for (const [input, port] of [["\n", 2206], ["3100\n", 3100], ["00080\n", 80], ["0\n65536\nabc\n1.5\n99999999999999999999\n65535\n", 65535]] as const) {
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${shellFunctions("valid_port", "prompt_port", "toml_escape", "write_config").replaceAll("/dev/tty", "/dev/null")}
          prompt_line() { IFS= read -r prompt_value; }
          chown() { :; }
          config_dir=$TEST_CONFIG_DIR
          config_file=$config_dir/config.toml
          database_url=sqlite://nodeflare.db
          admin_username=test-admin
          admin_password='TestPassword123'
          public_frontend_dir=$config_dir/frontend
          admin_frontend_dir=$config_dir/admin
          theme_dir=$config_dir/themes
          prompt_port
          write_config
        `], { input, env: { ...process.env, TEST_CONFIG_DIR: directory }, encoding: "utf8" });
        expect(result.status).toBe(0);
        const config = Bun.TOML.parse(readFileSync(join(directory, "config.toml"), "utf8")) as Record<string, unknown>;
        expect(config.bind_addr).toBe(`127.0.0.1:${port}`);
        expect(config.admin_password).toBe("TestPassword123");
        expect(config.database_url).toBe("sqlite://nodeflare.db");
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});

describe("server installer update output", () => {
  for (const healthy of [true, false]) {
    test(healthy ? "updates quietly without prompting or rewriting configuration" : "keeps startup diagnostics and never announces success on failure", () => {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-install-output-"));
      const config = join(directory, "config.toml");
      const original = 'bind_addr = "127.0.0.1:3100"\nadmin_password = ""\n';
      try {
        writeFileSync(config, original);
        const start = installer.lastIndexOf("\ndownload_release\n");
        if (start < 0) throw new Error("Missing installer entry point");
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${shellFunctions("log", "fail", "print_install_result")}
          config_dir=$TEST_DIRECTORY
          config_file=$config_dir/config.toml
          theme_dir=$config_dir/themes
          install_dir=$config_dir/install
          share_dir=$install_dir/share
          public_frontend_dir=$share_dir/frontend
          admin_frontend_dir=$share_dir/admin
          server_binary=$install_dir/nodeflare
          init_system=systemd
          download_release() { release_version=1.2.3; package_dir=$TEST_DIRECTORY/package; }
          prompt_credentials() { fail 'Unexpected credential prompt'; }
          prompt_port() { fail 'Unexpected port prompt'; }
          prompt_database() { fail 'Unexpected database prompt'; }
          write_config() { fail 'Unexpected config write'; }
          snapshot_install() { :; }
          stop_server() { :; }
          install_service() { :; }
          install() { :; }
          cp() { :; }
          rm() { :; }
          chown() { :; }
          chmod() { :; }
          start_server() { ${healthy ? "return 0" : "return 1"}; }
          journalctl() { printf 'startup diagnostic\\n'; }
          ${installer.slice(start)}
        `], { env: { ...process.env, TEST_DIRECTORY: directory }, encoding: "utf8" });
        expect(readFileSync(config, "utf8")).toBe(original);
        expect(result.stdout).not.toMatch(/NodeFlare|配置|沿用|复用|下一步|本机访问/);
        if (healthy) {
          expect(result.status).toBe(0);
          expect(result.stdout).toBe("正在更新至 v1.2.3\n正在备份程序文件\n正在停止旧服务\n正在替换程序文件\n正在启动服务\n\n更新完成（v1.2.3）\n");
          expect(result.stderr).toBe("");
        } else {
          expect(result.status).toBe(1);
          expect(result.stdout).not.toContain("完成");
          expect(result.stderr).toContain("startup diagnostic");
          expect(result.stderr).toContain("服务启动失败");
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

describe("server bundle installation", () => {
  for (const separateShare of [false, true]) {
    test(`replaces bundled resources without changing agent data (separate share=${separateShare})`, () => {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-bundle-install-"));
      const installDir = join(directory, "install");
      const shareDir = join(separateShare ? directory : installDir, "share");
      const configDir = join(directory, "config");
      const packageDir = join(directory, "package");
      try {
        for (const path of [installDir, shareDir, join(configDir, "agent"), join(configDir, "themes"), join(packageDir, "share", "frontend"), join(packageDir, "share", "admin")]) {
          mkdirSync(path, { recursive: true });
        }
        writeFileSync(join(installDir, "agent"), "agent-binary");
        writeFileSync(join(shareDir, "obsolete-resource"), "obsolete");
        writeFileSync(join(configDir, "config.toml"), 'bind_addr = "127.0.0.1:3100"\n');
        writeFileSync(join(configDir, "agent", "remote-tasks.json"), "agent-state");
        writeFileSync(join(configDir, "themes", "theme.txt"), "custom-theme");
        writeFileSync(join(packageDir, "nodeflare"), "server-binary");
        writeFileSync(join(packageDir, "share", "frontend", "index.html"), "public");
        writeFileSync(join(packageDir, "share", "admin", "admin.html"), "admin");
        const start = installer.lastIndexOf("\ndownload_release\n");
        if (start < 0) throw new Error("Missing installer entry point");
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${shellFunctions("log", "fail", "print_install_result")}
          config_dir=$TEST_DIRECTORY/config
          config_file=$config_dir/config.toml
          theme_dir=$config_dir/themes
          install_dir=$TEST_DIRECTORY/install
          share_dir=$TEST_SHARE_DIR
          public_frontend_dir=$share_dir/frontend
          admin_frontend_dir=$share_dir/admin
          server_binary=$install_dir/nodeflare
          download_release() { release_version=1.2.3; package_dir=$TEST_DIRECTORY/package; }
          snapshot_install() { :; }
          stop_server() { :; }
          install_service() { :; }
          chown() { :; }
          start_server() { return 0; }
          ${installer.slice(start)}
        `], { env: { ...process.env, TEST_DIRECTORY: directory, TEST_SHARE_DIR: shareDir }, encoding: "utf8" });
        expect(result.status).toBe(0);
        expect(existsSync(join(shareDir, "obsolete-resource"))).toBe(false);
        expect(readFileSync(join(shareDir, "frontend", "index.html"), "utf8")).toBe("public");
        expect(readFileSync(join(shareDir, "admin", "admin.html"), "utf8")).toBe("admin");
        expect(readFileSync(join(installDir, "nodeflare"), "utf8")).toBe("server-binary");
        expect(readFileSync(join(installDir, "agent"), "utf8")).toBe("agent-binary");
        expect(readFileSync(join(configDir, "config.toml"), "utf8")).toBe('bind_addr = "127.0.0.1:3100"\n');
        expect(readFileSync(join(configDir, "agent", "remote-tasks.json"), "utf8")).toBe("agent-state");
        expect(readFileSync(join(configDir, "themes", "theme.txt"), "utf8")).toBe("custom-theme");
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

describe("installer menu", () => {
  function route(input: string, args: string[] = []) {
    const dispatch = installer.slice(installer.indexOf("\nmode=menu"), installer.indexOf("\nfor required_command"));
    return spawnSync("sh", ["-c", `
      set -eu
      ${shellFunctions("show_menu", "confirm_purge", "fail", "usage").replaceAll("/dev/tty", "/dev/null")}
      server_binary=/nonexistent/nodeflare
      prompt_line() { IFS= read -r prompt_value; }
      id() { printf '0'; }
      detect_init_system() { printf systemd; }
      status_server() { printf 'status'; }
      restart_server() { printf 'restart'; }
      uninstall_server() { printf 'uninstall:%s' "$1"; }
      ${dispatch}
      printf 'install'
    `, "installer-test", ...args], { input, encoding: "utf8" });
  }

  test("routes choices and confirms uninstall while keeping data", () => {
    for (const [input, expected] of [["1\n", "install"], ["2\n", "status"], ["3\n", "restart"], ["4\ny\n", "uninstall:false"], ["4\n\n", ""], ["0\n", ""], ["\n", ""], ["invalid\n2\n", "status"]]) {
      const result = route(input);
      expect(result.status).toBe(0);
      expect(result.stdout).toBe(expected);
    }
  });

  test("explicit commands bypass the menu and invalid arguments fail", () => {
    for (const [args, expected] of [
      [["--install"], "install"], [["--status"], "status"], [["--restart"], "restart"],
      [["--uninstall"], "uninstall:false"], [["--uninstall", "--purge"], "uninstall:true"],
    ] as const) {
      const result = route("y\n", [...args]);
      expect(result.status).toBe(0);
      expect(result.stdout).toBe(expected);
    }
    expect(route("", ["--purge"]).status).toBe(1);
    expect(route("", ["--status", "--install"]).status).toBe(1);
  });

  test("purge cancellation never calls uninstall", () => {
    const result = route("n\n", ["--uninstall", "--purge"]);
    expect(result.status).toBe(1);
    expect(result.stdout).toBe("");
  });
});

describe("agent installer manual update", () => {
  test("Linux installation accepts a presence-only remote disable flag", () => {
    const functions = agentShellFunctions("parse_agent_args", "fail", "usage");
    for (const [args, expected] of [
      [[], "false"], [["--disable-remote"], "true"],
      [["--disable-remote", "false"], null], [["--disable-remote=true"], null],
      [["--disable-remote", "--disable-remote"], null],
    ] as const) {
      const result = spawnSync("sh", ["-c", `set -eu\n${functions}\nparse_agent_args "$@"\nprintf '%s' "$disable_remote"`, "agent-test", ...args], { encoding: "utf8" });
      expect(result.status).toBe(expected === null ? 1 : 0);
      if (expected !== null) expect(result.stdout).toBe(expected);
    }
  });

  test("parses the existing systemd and OpenRC service configuration", () => {
    const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-update-test-"));
    const systemd = join(directory, "nodeflare-agent.service");
    const openrc = join(directory, "nodeflare-agent.openrc");
    try {
      const functions = agentShellFunctions("load_installed_agent_config");
      for (const position of ["absent", "before", "after"]) {
        const prefix = position === "before" ? "--disable-remote " : "";
        const suffix = position === "after" ? " --disable-remote" : "";
        writeFileSync(systemd, [
          "[Service]",
          `ExecStart=/opt/nodeflare/agent ${prefix}-e https://monitor.example.com -i 120${suffix}`,
          "Environment=NODEFLARE_AGENT_TOKEN=token-systemd",
          "",
        ].join("\n"));
        writeFileSync(openrc, [
          `command_args="${prefix}-e https://monitor.example.com -i 45${suffix}"`,
          "export NODEFLARE_AGENT_TOKEN=\"token-openrc\"",
          "",
        ].join("\n"));
        for (const [init, path, interval] of [["systemd", systemd, 120], ["openrc", openrc, 45]] as const) {
          const result = spawnSync("sh", ["-c", `
            set -eu
            ${functions}
            init_system=${init}
            SERVICE_FILE=$SERVICE_CONFIG
            OPENRC_FILE=$SERVICE_CONFIG
            load_installed_agent_config
            printf '%s|%s|%s|%s\\n' "$endpoint" "$token" "$interval" "$disable_remote"
          `], { env: { ...process.env, SERVICE_CONFIG: path }, encoding: "utf8" });
          expect(result.status).toBe(0);
          expect(result.stdout).toBe(`https://monitor.example.com|token-${init}|${interval}|${position !== "absent"}\n`);
        }
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test("accepts only the optional download mirror", () => {
    expect(spawnSync("sh", ["-n"], { input: agentInstaller }).status).toBe(0);
    const functions = agentShellFunctions("update_agent", "load_installed_agent_config", "fail", "usage");
    for (const disabled of [true, false]) {
      for (const mirror of [[], ["-m", "https://gh-proxy.com"]]) {
        const accepted = spawnSync("sh", ["-c", `
          set -eu
          ${functions}
          id() { printf '0'; }
          detect_init_system() { printf systemd; }
          load_installed_agent_config() {
            endpoint=https://monitor.example.com
            token=token
            interval=120
            disable_remote=${disabled}
          }
          install_agent() { printf '%s\\n' "$@"; }
          update_agent "$@"
        `, "agent-test", ...mirror], { encoding: "utf8" });
        expect(accepted.status).toBe(0);
        expect(accepted.stdout.trim().split("\n")).toEqual([
          "-e", "https://monitor.example.com", "-t", "token", "-i", "120", ...mirror,
          ...(disabled ? ["--disable-remote"] : []),
        ]);
      }
    }

    const rejected = spawnSync("sh", ["-c", `
      set -eu
      ${functions}
      id() { printf '0'; }
      update_agent --bad
    `], { encoding: "utf8" });
    expect(rejected.status).toBe(1);
  });
});

describe("agent installer service safety", () => {
  test("a failed stop of an existing service prevents replacement", () => {
    const stop = agentInstaller.slice(agentInstaller.indexOf("  stop_agent || fail"), agentInstaller.indexOf('  mv "$temporary" "$AGENT_FILE"'));
    expect(stop).toContain("stop_agent");
    const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-stop-"));
    const log = join(directory, "stop.log");
    try {
      writeFileSync(log, "existing-service");
      // Dash rejects hyphenated function names, so mock the external commands.
      for (const command of ["systemctl", "rc-service"]) {
        writeFileSync(join(directory, command), '#!/bin/sh\nprintf \'%s\\n\' "$*" > "$TEST_STOP_LOG"\nexit 1\n', { mode: 0o755 });
      }
      for (const init of ["systemd", "openrc"]) {
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${agentShellFunctions("fail", "stop_agent")}
          init_system=$TEST_INIT
          had_service=true
          SERVICE_NAME=nodeflare-agent
          SERVICE_FILE=$TEST_STOP_LOG
          OPENRC_FILE=$TEST_STOP_LOG
          ${stop}
          printf 'replaced'
        `], { env: { ...process.env, PATH: `${directory}${delimiter}${process.env.PATH ?? ""}`, TEST_INIT: init, TEST_STOP_LOG: log }, encoding: "utf8" });
        expect(result.status).toBe(1);
        expect(result.stdout).toBe("");
        expect(result.stderr).toContain("未替换程序");
        expect(readFileSync(log, "utf8")).toBe(init === "systemd" ? "stop nodeflare-agent\n" : "nodeflare-agent stop\n");
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test("refuses to stop its own systemd service from remote execution", () => {
    const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-cgroup-"));
    const path = join(directory, "cgroup");
    try {
      for (const [cgroup, safe] of [
        ["0::/system.slice/nodeflare-agent.service\n", false],
        ["1:name=systemd:/system.slice/nodeflare-agent.service/remote\n", false],
        ["0::/user.slice/session-10.scope\n", true],
        ["0::/system.slice/nodeflare.service\n", true],
        ["0::/system.slice/other-nodeflare-agent.service\n", true],
      ] as const) {
        writeFileSync(path, cgroup);
        const result = spawnSync("sh", ["-c", `
          set -eu
          init_system=systemd
          SERVICE_NAME=nodeflare-agent
          ${agentShellFunctions("fail", "ensure_not_agent_service").replaceAll('"/proc/$$/cgroup"', '"$TEST_CGROUP_FILE"')}
          ensure_not_agent_service
          printf 'safe-to-stop'
        `], { env: { ...process.env, TEST_CGROUP_FILE: path }, encoding: "utf8" });
        expect(result.status).toBe(safe ? 0 : 1);
        expect(result.stdout).toBe(safe ? "safe-to-stop" : "");
        if (!safe) expect(result.stderr).toContain("服务未停止");
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  for (const file of ["../../install.sh", "../../agent/agent.sh", "../../agent/install-macos.sh", "../../agent/install-freebsd.sh"]) {
    test(`${file} exits on termination and runs rollback once`, () => {
      const source = readFileSync(new URL(file, import.meta.url), "utf8");
      const traps = source.match(/^[ \t]*trap (?:cleanup|cleanup_agent_install) EXIT\n[ \t]*trap 'exit 129' HUP\n[ \t]*trap 'exit 130' INT\n[ \t]*trap 'exit 143' TERM/m)?.[0];
      expect(traps).toBeDefined();
      const result = spawnSync("sh", ["-c", `
        cleanup() { printf 'rollback'; }
        cleanup_agent_install() { cleanup; }
        ${traps}
        kill -TERM "$$"
        printf 'continued-after-termination'
      `], { encoding: "utf8" });
      expect(result.status).toBe(143);
      expect(result.stdout).toBe("rollback");
    });
  }
});
