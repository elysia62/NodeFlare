import { describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { delimiter, join } from "node:path";
import { tmpdir } from "node:os";

const serverSource = readFileSync(new URL("../../install.sh", import.meta.url), "utf8");

function shellFunctions(source: string, ...names: string[]) {
  return names.map((name) => {
    const body = source.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"))?.[0];
    if (!body) throw new Error(`Missing shell function: ${name}`);
    return body;
  }).join("\n");
}

function powershellFunction(source: string, name: string) {
  const body = source.match(new RegExp(`^function ${name} \\{[\\s\\S]*?^\\}`, "m"))?.[0];
  if (!body) throw new Error(`Missing PowerShell function: ${name}`);
  return body;
}

describe("updates wait for the previous service to stop", () => {
  const cases = [
    ["server systemd", "../../install.sh", "stop_server", "systemd"],
    ["server OpenRC", "../../install.sh", "stop_server", "openrc"],
    ["server launchd", "../../install.sh", "stop_server", "launchd"],
    ["server FreeBSD", "../../install.sh", "stop_server", "freebsd"],
    ["agent systemd", "../../agent/agent.sh", "stop_agent", "systemd"],
    ["agent OpenRC", "../../agent/agent.sh", "stop_agent", "openrc"],
    ["agent launchd", "../../agent/install-macos.sh", "stop_agent", "launchd"],
    ["agent FreeBSD", "../../agent/install-freebsd.sh", "stop_agent", "freebsd"],
  ] as const;

  for (const [label, path, stop, init] of cases) {
    const source = readFileSync(new URL(path, import.meta.url), "utf8");
    for (const mode of ["stops", "stop-fails", "still-running"] as const) {
      test(`${label}: ${mode}`, () => {
        const directory = mkdtempSync(join(tmpdir(), "nodeflare-stop-test-"));
        try {
          const mock = `#!/bin/sh
set -eu
case "$0" in */systemctl|*/launchctl) action=$1 ;; *) action=$2 ;; esac
case "$action" in
  show) if [ "$(cat "$TEST_STATE")" = running ]; then printf '123'; else printf '0'; fi ;;
  status|onestatus|print) [ "$(cat "$TEST_STATE")" = running ] ;;
  stop|onestop|bootout)
    [ "$TEST_MODE" != stop-fails ] || exit 1
    if [ "$TEST_MODE" = stops ]; then printf stopped > "$TEST_STATE"; fi
    ;;
  *) exit 1 ;;
esac
`;
          for (const command of ["systemctl", "rc-service", "launchctl", "service"]) {
            writeFileSync(join(directory, command), mock, { mode: 0o755 });
          }
          writeFileSync(join(directory, "state"), "running");
          writeFileSync(join(directory, "service-definition"), "existing-service");
          const result = spawnSync("sh", ["-c", `
            set -eu
            ${shellFunctions(source, ...(stop === "stop_server" ? ["service_definition", stop] : [stop]))}
            init_system=$TEST_INIT
            SERVICE_NAME=nodeflare-agent
            LABEL=nodeflare-agent
            SERVICE_FILE=$TEST_DIRECTORY/service-definition
            OPENRC_FILE=$SERVICE_FILE
            PLIST_FILE=$SERVICE_FILE
            systemd_file=$SERVICE_FILE
            openrc_file=$SERVICE_FILE
            launchd_file=$SERVICE_FILE
            freebsd_rc_file=$SERVICE_FILE
            if ${stop}; then printf replaced; else exit 1; fi
          `], {
            env: { ...process.env, PATH: `${directory}${delimiter}${process.env.PATH ?? ""}`,
              TEST_DIRECTORY: directory, TEST_STATE: join(directory, "state"), TEST_MODE: mode, TEST_INIT: init },
            encoding: "utf8", timeout: 5000,
          });
          expect(result.status).toBe(mode === "stops" ? 0 : 1);
          expect(result.stdout).toBe(mode === "stops" ? "replaced" : "");
        } finally {
          rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});

describe("server update rollback ownership", () => {
  for (const separateShare of [false, true]) {
    test(`preserves newer Agent files during rollback (separate share=${separateShare})`, () => {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-rollback-test-"));
      const install = join(directory, "install");
      const share = separateShare ? join(directory, "share") : join(install, "share");
      try {
        mkdirSync(share, { recursive: true });
        mkdirSync(install, { recursive: true });
        writeFileSync(join(install, "nodeflare"), "previous-server");
        writeFileSync(join(install, "agent"), "previous-agent");
        writeFileSync(join(share, "index.html"), "previous-frontend");
        writeFileSync(join(directory, "config.toml"), "server-config");
        writeFileSync(join(directory, "service"), "previous-service");
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${shellFunctions(serverSource, "log", "snapshot_install", "rollback_install", "service_definition")}
          install_dir=$TEST_DIRECTORY/install
          server_binary=$install_dir/nodeflare
          share_dir=$TEST_SHARE
          config_file=$TEST_DIRECTORY/config.toml
          download_dir=$TEST_DIRECTORY/download
          systemd_file=$TEST_DIRECTORY/service
          init_system=systemd
          previous_install=false
          previous_share=false
          previous_service=false
          config_created=false
          snapshot_install
          printf newer-agent > "$install_dir/agent"
          printf live-state > "$install_dir/pending.jsonl"
          printf broken-server > "$server_binary"
          printf broken-frontend > "$share_dir/index.html"
          stop_server() { return 0; }
          start_server() { return 0; }
          rollback_install
        `], { env: { ...process.env, TEST_DIRECTORY: directory, TEST_SHARE: share }, encoding: "utf8" });
        expect(result.status).toBe(0);
        expect(readFileSync(join(install, "nodeflare"), "utf8")).toBe("previous-server");
        expect(readFileSync(join(share, "index.html"), "utf8")).toBe("previous-frontend");
        expect(readFileSync(join(install, "agent"), "utf8")).toBe("newer-agent");
        expect(readFileSync(join(install, "pending.jsonl"), "utf8")).toBe("live-state");
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }

  test("failed rollback preserves its backup and does not overwrite a running service", () => {
    const directory = mkdtempSync(join(tmpdir(), "nodeflare-rollback-failure-"));
    try {
      mkdirSync(join(directory, "download", "previous"), { recursive: true });
      writeFileSync(join(directory, "download", "previous", "nodeflare"), "previous-server");
      writeFileSync(join(directory, "nodeflare"), "running-server");
      const result = spawnSync("sh", ["-c", `
        set -eu
        ${shellFunctions(serverSource, "cleanup", "rollback_install", "log")}
        restore_tty() { return 0; }
        stop_server() { return 1; }
        rollback_ready=true
        previous_install=true
        server_binary=$TEST_DIRECTORY/nodeflare
        config_temp=""
        download_dir=$TEST_DIRECTORY/download
        backup_dir=$download_dir/previous
        cleanup
      `], { env: { ...process.env, TEST_DIRECTORY: directory }, encoding: "utf8" });
      expect(result.status).toBe(0);
      expect(result.stderr).toContain("备份已保留");
      expect(readFileSync(join(directory, "nodeflare"), "utf8")).toBe("running-server");
      expect(existsSync(join(directory, "download", "previous", "nodeflare"))).toBe(true);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});

describe("Agent rollback backup retention", () => {
  for (const path of ["../../agent/agent.sh", "../../agent/install-macos.sh", "../../agent/install-freebsd.sh"]) {
    test(`${path} preserves backups when restoring a file fails`, () => {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-rollback-"));
      try {
        for (const name of ["agent", "previous-agent", "previous-service", "download"]) {
          writeFileSync(join(directory, name), name);
        }
        const source = readFileSync(new URL(path, import.meta.url), "utf8");
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${shellFunctions(source, "log", "cleanup_agent_install")}
          stop_agent() { return 0; }
          cp() { return 1; }
          systemctl() { return 0; }
          launchctl() { return 0; }
          service() { return 0; }
          init_system=systemd
          rollback_agent=true
          had_agent=true
          had_service=true
          SERVICE_NAME=fixture
          STATE_DIR=$TEST_DIRECTORY
          AGENT_FILE=$TEST_DIRECTORY/agent
          SERVICE_FILE=$TEST_DIRECTORY/service
          PLIST_FILE=$SERVICE_FILE
          backup_agent=$TEST_DIRECTORY/previous-agent
          backup_service=$TEST_DIRECTORY/previous-service
          temporary=$TEST_DIRECTORY/download
          if cleanup_agent_install; then exit 0; else exit 1; fi
        `], { env: { ...process.env, TEST_DIRECTORY: directory }, encoding: "utf8" });
        expect(result.status).toBe(1);
        expect(readFileSync(join(directory, "agent"), "utf8")).toBe("agent");
        expect(existsSync(join(directory, "previous-agent"))).toBe(true);
        expect(existsSync(join(directory, "previous-service"))).toBe(true);
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

describe("Windows update process lifecycle", () => {
  const powershell = Bun.which("pwsh");
  for (const kind of ["server", "agent"] as const) {
    const source = readFileSync(new URL(kind === "server" ? "../../install.ps1" : "../../agent/install.ps1", import.meta.url), "utf8");
    const stop = kind === "server" ? "Stop-Server" : "Stop-Agent";
    for (const mode of ["stops", "stop-fails", "still-running", "query-fails"] as const) {
      test.skipIf(!powershell)(`${kind}: ${mode}`, () => {
        const result = spawnSync(powershell!, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", `
          $ErrorActionPreference = "Stop"
          $TaskName = "fixture"
          $ServerFile = $AgentFile = "/fixture/nodeflare.exe"
          $script:Stopped = $false
          $script:Ticks = 0
          function Get-ScheduledTask {
            [pscustomobject]@{ State = $(if ($script:Stopped) { "Ready" } else { "Running" }) }
          }
          function Stop-ScheduledTask {
            if ($env:TEST_MODE -eq "stop-fails") { throw "Stop failed" }
            $script:Stopped = $true
          }
          function Get-CimInstance {
            if ($env:TEST_MODE -eq "query-fails") { throw "Process query failed" }
            [pscustomobject]@{ ExecutablePath = "/another-program/nodeflare.exe" }
            if ($script:Ticks -lt 2 -or $env:TEST_MODE -eq "still-running") {
              [pscustomobject]@{ ExecutablePath = $ServerFile }
            }
          }
          function Start-Sleep { $script:Ticks++ }
          function Stop-Install([string]$Message) { throw $Message }
          function Write-InstallError([string]$Message) { throw $Message }
          ${powershellFunction(source, stop)}
          try { ${stop}; Write-Output "replaced:$script:Ticks" } catch { Write-Output blocked; exit 1 }
        `], { env: { ...process.env, TEST_MODE: mode }, encoding: "utf8", timeout: 10_000 });
        expect(result.status).toBe(mode === "stops" ? 0 : 1);
        expect(result.stdout.trim()).toBe(mode === "stops" ? "replaced:2" : "blocked");
      });
    }
  }

  test.skipIf(!powershell)("server rollback retains Agent binaries and launchers", () => {
    const directory = mkdtempSync(join(tmpdir(), "nodeflare-windows-rollback-"));
    try {
      for (const path of ["install/share", "backup/share"]) mkdirSync(join(directory, path), { recursive: true });
      for (const [path, value] of Object.entries({
        "install/nodeflare.exe": "broken-server", "install/share/index.html": "broken-frontend",
        "install/agent.exe": "newer-agent", "install/run-agent.ps1": "newer-launcher",
        "backup/nodeflare.exe": "previous-server", "backup/share/index.html": "previous-frontend",
      })) writeFileSync(join(directory, path), value);
      const source = readFileSync(new URL("../../install.ps1", import.meta.url), "utf8");
      const result = spawnSync(powershell!, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", `
        $ErrorActionPreference = "Stop"
        $InstallDir = Join-Path $env:TEST_DIRECTORY "install"
        $ServerFile = Join-Path $InstallDir "nodeflare.exe"
        $ShareDir = Join-Path $InstallDir "share"
        $PreviousInstall = Join-Path $env:TEST_DIRECTORY "backup"
        $HadPreviousInstall = $HadPreviousShare = $true
        ${powershellFunction(source, "Restore-ServerFiles")}
        Restore-ServerFiles
      `], { env: { ...process.env, TEST_DIRECTORY: directory }, encoding: "utf8", timeout: 10_000 });
      expect(result.status).toBe(0);
      expect(readFileSync(join(directory, "install", "nodeflare.exe"), "utf8")).toBe("previous-server");
      expect(readFileSync(join(directory, "install", "share", "index.html"), "utf8")).toBe("previous-frontend");
      expect(readFileSync(join(directory, "install", "agent.exe"), "utf8")).toBe("newer-agent");
      expect(readFileSync(join(directory, "install", "run-agent.ps1"), "utf8")).toBe("newer-launcher");
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
