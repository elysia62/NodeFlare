import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

function shellFunction(script: string, name: string) {
  const match = script.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
  if (!match) throw new Error(`Missing shell function: ${name}`);
  return match[0];
}

for (const [name, path, server] of [
  ["server", "../../install.sh", true],
  ["Linux agent", "../../agent/agent.sh", false],
  ["macOS agent", "../../agent/install-macos.sh", false],
  ["FreeBSD agent", "../../agent/install-freebsd.sh", false],
] as const) {
  const script = readFileSync(new URL(path, import.meta.url), "utf8");
  function output(command: string, newConfig = true, update = false, hadAgent = false, init = "systemd") {
    return spawnSync("sh", ["-c", `
      set -eu
      ${["log", "fail", "print_install_result"].map(fn => shellFunction(script, fn)).join("\n")}
      new_config=${newConfig}
      update=${update}
      had_agent=${hadAgent}
      release_version=1.2.3
      installed_version=1.2.3
      config_dir=/test/config
      server_binary=/test/nodeflare
      server_port=3100
      SERVICE_NAME=nodeflare-agent
      LABEL=nodeflare-agent
      init_system=${init}
      ${command}
    `], { encoding: "utf8" });
  }

  describe(`${name} installer output`, () => {
    test("valid shell syntax and no repeated branding prefix", () => {
      expect(spawnSync("sh", ["-n"], { input: script }).status).toBe(0);
      expect(script).not.toContain("[NodeFlare]");
      const result = output('log "正在检查运行环境"');
      expect(result.status).toBe(0);
      expect(result.stdout).toBe("正在检查运行环境\n");
    });

    test("keeps first-install guidance and displays the version once", () => {
      for (const init of ["systemd", "openrc"]) {
        const result = output("print_install_result", true, false, false, init);
        expect(result.status).toBe(0);
        expect(result.stdout).toStartWith("\n安装完成（v1.2.3）\n");
        expect(result.stdout.match(/1\.2\.3/g)?.length).toBe(1);
        expect(result.stdout).toContain(server ? "本机访问：http://127.0.0.1:3100/admin/login" : "查看状态：");
        if (server) expect(result.stdout).toContain("下一步：");
      }
    });

    test("updates only print the result and version", () => {
      const states = server ? [[false, false]] : [[true, false], [false, true], [true, true]];
      for (const [update, hadAgent] of states) {
        const result = output("print_install_result", false, update, hadAgent);
        expect(result.status).toBe(0);
        expect(result.stdout).toBe("\n更新完成（v1.2.3）\n");
        expect(result.stderr).toBe("");
      }
    });

    test("errors remain visible and return failure", () => {
      const result = output('fail "服务启动失败"');
      expect(result.status).toBe(1);
      expect(result.stdout).toBe("");
      expect(result.stderr).toBe("错误：服务启动失败\n");
    });
  });
}

const powershell = Bun.which("pwsh");
for (const [name, path, server] of [
  ["Windows server", "../../install.ps1", true],
  ["Windows agent", "../../agent/install.ps1", false],
] as const) {
  const file = fileURLToPath(new URL(path, import.meta.url));
  const script = readFileSync(file, "utf8");
  const functions = ["Write-Step", "Show-InstallResult"].map(name => {
    const match = script.match(new RegExp(`^function ${name}(?:\\([^\\n]*\\))? \\{[\\s\\S]*?^\\}`, "m"));
    if (!match) throw new Error(`Missing PowerShell function: ${name}`);
    return match[0];
  }).join("\n");

  describe(`${name} installer output`, () => {
    test("does not contain a repeated branding prefix", () => {
      expect(script).not.toContain("[NodeFlare]");
    });

    test.skipIf(!powershell)("parses with PowerShell", () => {
      const result = spawnSync(powershell!, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", `
        $tokens = $null
        $errors = $null
        [System.Management.Automation.Language.Parser]::ParseFile($env:TEST_SCRIPT, [ref]$tokens, [ref]$errors) > $null
        if ($errors.Count) { $errors | ForEach-Object { Write-Error $_ }; exit 1 }
      `], { env: { ...process.env, TEST_SCRIPT: file }, encoding: "utf8" });
      expect(result.status).toBe(0);
    });

    test.skipIf(!powershell)("separates fresh-install guidance from update results", () => {
      const states = server ? [[true, false, false], [false, false, false]] : [
        [true, false, false], [false, true, false], [false, false, true], [false, true, true],
      ];
      for (const [fresh, update, hadAgent] of states) {
        const result = spawnSync(powershell!, ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", `
          $ErrorActionPreference = "Stop"
          Set-StrictMode -Version Latest
          ${functions}
          $NewConfig = $${fresh}
          $Update = $${update}
          $HadPreviousAgent = $${hadAgent}
          $Version = "1.2.3"
          $InstalledVersion = "1.2.3"
          $DataDir = "C:/test/data"
          $TaskName = "test-service"
          $Port = 3100
          Write-Step "正在启动服务"
          Show-InstallResult
        `], { encoding: "utf8" });
        expect(result.status).toBe(0);
        const output = result.stdout.replaceAll("\r\n", "\n");
        if (fresh) {
          expect(output).toContain("安装完成（v1.2.3）");
          expect(output).toContain(server ? "本机访问：http://127.0.0.1:3100/admin/login" : "查看状态：");
        } else {
          expect(output).toBe("正在启动服务\n\n更新完成（v1.2.3）\n");
        }
      }
    });
  });
}
