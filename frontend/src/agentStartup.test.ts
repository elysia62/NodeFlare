import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const installers = {
  systemd: "../../agent/agent.sh",
  openrc: "../../agent/agent.sh",
  macos: "../../agent/install-macos.sh",
  freebsd: "../../agent/install-freebsd.sh",
};

const scenarios = [
  ["healthy", 0, 1],
  ["delayed", 0, 4],
  ["missing", 1, 29],
  ["invalid", 1, 29],
  ["exited", 1, 1],
  ["marker-lost", 1, 1],
  ["restarted", 1, 1],
  ["wrong-process", 1, 0],
  ["service-dead", 1, 0],
  ["stale-pid", 1, 0],
  ["deadline", 1, 1],
] as const;

for (const [platform, path] of Object.entries(installers)) {
  const source = readFileSync(new URL(path, import.meta.url), "utf8");
  const functions = ["agent_service_ready", "verify_agent_started"].map((name) => {
    const match = source.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
    if (!match) throw new Error(`Missing ${platform} function: ${name}`);
    return match[0];
  }).join("\n");

  describe(`${platform} Agent local readiness`, () => {
    for (const [scenario, expectedStatus, expectedSleeps] of scenarios) {
      test(`${scenario}: local readiness, identity and bounded stable window`, () => {
        const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-ready-"));
        try {
          if (scenario !== "missing" && scenario !== "delayed") {
            writeFileSync(join(directory, "ready.pid"), scenario === "invalid" ? "not-a-pid\n" : "321\n");
          }
          writeFileSync(join(directory, "child.pid"), scenario === "wrong-process" ? "654\n" : "321\n");
          const result = spawnSync("sh", ["-c", `
            set -eu
            STATE_DIR=$TEST_DIRECTORY
            AGENT_PID_FILE=$TEST_DIRECTORY/child.pid
            AGENT_FILE=/opt/nodeflare/agent
            SERVICE_NAME=nodeflare-agent
            LABEL=nodeflare-agent
            init_system=$TEST_PLATFORM
            tick=0
            date() {
              if [ "$TEST_SCENARIO" = deadline ] && [ "$tick" -gt 0 ]; then printf '1031';
              else printf '%s' "$((1000 + tick))"; fi
            }
            sleep() {
              [ "$1" = 1 ] || return 90
              tick=$((tick + 1))
              if [ "$TEST_SCENARIO" = delayed ] && [ "$tick" = 3 ]; then printf '321\n' > "$STATE_DIR/ready.pid"; fi
              if [ "$TEST_SCENARIO" = marker-lost ]; then rm -f "$STATE_DIR/ready.pid"; fi
              if [ "$TEST_SCENARIO" = restarted ]; then
                printf '654\n' > "$STATE_DIR/ready.pid"
                printf '654\n' > "$AGENT_PID_FILE"
              fi
            }
            kill() {
              [ "$1" = -0 ] || return 91
              [ "$TEST_SCENARIO" != stale-pid ] || return 1
              [ "$TEST_SCENARIO" != exited ] || [ "$tick" = 0 ]
            }
            service_pid() {
              if [ "$TEST_SCENARIO" = wrong-process ] || { [ "$TEST_SCENARIO" = restarted ] && [ "$tick" -gt 0 ]; }; then printf '654';
              else printf '321'; fi
            }
            systemctl() {
              case "$1" in
                is-active) [ "$TEST_SCENARIO" != service-dead ] ;;
                show) service_pid ;;
                *) return 92 ;;
              esac
            }
            rc-service() { [ "$TEST_SCENARIO" != service-dead ]; }
            readlink() {
              if [ "$TEST_SCENARIO" = wrong-process ]; then printf '/other/agent'; else printf '%s' "$AGENT_FILE"; fi
            }
            launchctl() {
              [ "$TEST_SCENARIO" != service-dead ] || return 1
              printf 'state = running\npid = %s\n' "$(service_pid)"
            }
            service() { [ "$TEST_SCENARIO" != service-dead ]; }
            ${functions}
            if verify_agent_started; then result=0; else result=1; fi
            printf '%s' "$tick"
            exit "$result"
          `], {
            env: { ...process.env, TEST_DIRECTORY: directory, TEST_PLATFORM: platform, TEST_SCENARIO: scenario },
            encoding: "utf8", timeout: 5_000,
          });
          expect(result.error).toBeUndefined();
          expect(result.status).toBe(expectedStatus);
          expect(result.stdout).toBe(String(expectedSleeps));
        } finally {
          rmSync(directory, { recursive: true, force: true });
        }
      });
    }

    test("clears stale readiness only after stopping the old service, before replacing the binary", () => {
      const stop = source.lastIndexOf('stop_agent || fail "无法停止已有 Agent 服务，未替换程序"');
      const clear = source.indexOf('rm -f "$STATE_DIR/ready.pid"', stop);
      const replace = source.indexOf('mv "$temporary" "$AGENT_FILE"', stop);
      expect(stop).toBeGreaterThan(0);
      expect(clear).toBeGreaterThan(stop);
      expect(replace).toBeGreaterThan(clear);
      expect(source).toContain("verify_agent_started ||");
      expect(spawnSync("sh", ["-n"], { input: source }).status).toBe(0);
    });
  });
}

test("FreeBSD checks the daemon child, keeping the supervisor PID for stop/restart", () => {
  const source = readFileSync(new URL(installers.freebsd, import.meta.url), "utf8");
  expect(source).toContain('command_args="-P \\${pidfile} -p $AGENT_PID_FILE');
  expect(source).toContain('current_pid=$(cat "$AGENT_PID_FILE"');
});

describe("Windows Agent local readiness", () => {
  const source = readFileSync(new URL("../../agent/install.ps1", import.meta.url), "utf8");
  const powershell = Bun.which("pwsh");
  const functions = ["Wait-Agent", "Write-InstallError"].map((name) => {
    const match = source.match(new RegExp(`^function ${name}[^\\n]*\\{[\\s\\S]*?^\\}`, "m"));
    if (!match) throw new Error(`Missing Windows function: ${name}`);
    return match[0];
  }).join("\n");

  for (const [scenario, expectedStatus, expectedSleeps] of scenarios.filter(([scenario]) => scenario !== "deadline")) {
    test.skipIf(!powershell)(`${scenario}: validates the actual executable, not only the PowerShell task`, () => {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-ready-windows-"));
      try {
        if (scenario !== "missing" && scenario !== "delayed") {
          writeFileSync(join(directory, "ready.pid"), scenario === "invalid" ? "not-a-pid\n" : "321\n");
        }
        const result = spawnSync(powershell!, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", `
          $ErrorActionPreference = "Stop"
          $TaskName = "nodeflare-agent"
          $AgentFile = Join-Path $env:TEST_DIRECTORY "agent.exe"
          $ReadyFile = Join-Path $env:TEST_DIRECTORY "ready.pid"
          $script:Tick = 0
          function Start-Sleep {
            param([int]$Seconds)
            if ($Seconds -ne 1) { throw "Unexpected sleep" }
            $script:Tick++
            if ($env:TEST_SCENARIO -eq "delayed" -and $script:Tick -eq 3) { Set-Content -LiteralPath $ReadyFile -Value "321" }
            if ($env:TEST_SCENARIO -eq "marker-lost") { Remove-Item -LiteralPath $ReadyFile -Force }
            if ($env:TEST_SCENARIO -eq "restarted") { Set-Content -LiteralPath $ReadyFile -Value "654" }
          }
          function Get-ScheduledTask {
            param([string]$TaskName, [string]$ErrorAction)
            $State = if ($env:TEST_SCENARIO -eq "service-dead") { "Ready" } else { "Running" }
            [pscustomobject]@{ State = $State }
          }
          function Get-CimInstance {
            param([string]$ClassName, [string]$Filter, [string]$ErrorAction)
            if ($ClassName -ne "Win32_Process" -or $Filter -notmatch '^ProcessId = (321|654)$') { throw "Unsafe process lookup" }
            if ($env:TEST_SCENARIO -eq "stale-pid" -or ($env:TEST_SCENARIO -eq "exited" -and $script:Tick -gt 0)) { return }
            $Executable = if ($env:TEST_SCENARIO -eq "wrong-process") { "other-agent.exe" } else { $AgentFile }
            [pscustomobject]@{ ExecutablePath = $Executable }
          }
          ${functions}
          try { Wait-Agent; Write-Output $script:Tick; exit 0 }
          catch { Write-Output $script:Tick; exit 1 }
        `], { env: { ...process.env, TEST_DIRECTORY: directory, TEST_SCENARIO: scenario }, encoding: "utf8", timeout: 10_000 });
        expect(result.error).toBeUndefined();
        expect(result.status).toBe(expectedStatus);
        expect(result.stdout.trim()).toBe(String(expectedSleeps));
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }

  test("checks a protected fresh marker and real process identity before confirming installation", () => {
    const stop = source.indexOf("  Stop-Agent\n  $InstallChanged = $true");
    const clear = source.indexOf("Remove-Item -LiteralPath $ReadyFile -Force -ErrorAction Stop", stop);
    const replace = source.indexOf("Move-Item -LiteralPath $Temporary", stop);
    expect(stop).toBeGreaterThan(0);
    expect(clear).toBeGreaterThan(stop);
    expect(replace).toBeGreaterThan(clear);
    expect(source).toContain('$env:NODEFLARE_STATE_DIR = Split-Path -Parent $ConfigFile');
    expect(source).toContain('Where-Object { $_.ExecutablePath -eq $AgentFile }');
    expect(source).toContain("  Wait-Agent\n  $InstallChanged = $false");
  });
});
