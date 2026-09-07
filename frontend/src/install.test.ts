import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const installer = readFileSync(new URL("../../install.sh", import.meta.url), "utf8");
const agentInstaller = readFileSync(new URL("../../agent/agent.sh", import.meta.url), "utf8");

test("CI checks each shell script and stops on a later syntax error", () => {
  const workflow = Bun.YAML.parse(readFileSync(new URL("../../.github/workflows/release.yml", import.meta.url), "utf8")) as {
    jobs: { quality: { steps: { name?: string; run?: string }[] } };
  };
  const command = workflow.jobs.quality.steps.find((step) => step.name === "Check shell scripts")?.run;
  expect(command).toBeDefined();
  const result = spawnSync("sh", ["-c", `
    sh() {
      [ "$#" -eq 2 ] && [ "$1" = -n ] || return 98
      printf '%s\\n' "$2"
      [ "$2" != agent/agent.sh ] || return 19
    }
    ${command}
  `], { encoding: "utf8" });
  expect(result.status).toBe(19);
  expect(result.stdout).toBe("install.sh\nagent/agent.sh\n");
});

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

const startFunction = shellFunctions("start_server");

function start(scenario: string) {
  return spawnSync("sh", ["-c", `
    set -eu
    init_system=systemd
    tick=0
    sleep() { tick=$((tick + 1)); }
    systemctl() {
      case "$1" in
        daemon-reload) [ "$SCENARIO" != command-failure ] ;;
        enable|restart) return 0 ;;
        is-active) [ "$SCENARIO" != exits ] || [ "$tick" -lt 2 ] ;;
        show)
          if [ "$SCENARIO" = restarts ] && [ "$tick" -ge 2 ]; then
            printf '456\n'
          else
            printf '123\n'
          fi ;;
        *) return 99 ;;
      esac
    }
    ${startFunction}
    if start_server; then printf 'ready:%s\n' "$tick"; else exit 1; fi
  `], { env: { ...process.env, SCENARIO: scenario }, encoding: "utf8" });
}

describe("installer startup verification", () => {
  test("waits for the same process to remain active", () => {
    const result = start("healthy");
    expect(result.status).toBe(0);
    expect(result.stdout).toBe("ready:10\n");
  });
  for (const scenario of ["exits", "restarts", "command-failure"]) {
    test(`rejects ${scenario} even when called in an if condition`, () => {
      expect(start(scenario).status).toBe(1);
    });
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
          agent_installer_dir=$config_dir/agent
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
          agent_installer_dir=$share_dir/agent
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
          expect(result.stdout).toBe("正在更新至 v1.2.3\n正在启动服务\n\n更新完成（v1.2.3）\n");
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
  test("parses the existing systemd and OpenRC service configuration", () => {
    const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-update-test-"));
    const systemd = join(directory, "nodeflare-agent.service");
    const openrc = join(directory, "nodeflare-agent.openrc");
    try {
      writeFileSync(systemd, [
        "[Service]",
        "ExecStart=/opt/nodeflare/agent -e https://monitor.example.com -i 120",
        "Environment=NODEFLARE_AGENT_TOKEN=token-systemd",
        "",
      ].join("\n"));
      writeFileSync(openrc, [
        "command_args=\"-e https://monitor.example.com -i 45\"",
        "export NODEFLARE_AGENT_TOKEN=\"token-openrc\"",
        "",
      ].join("\n"));
      const functions = agentShellFunctions("load_installed_agent_config");
      for (const [init, path, expected] of [
        ["systemd", systemd, "https://monitor.example.com|token-systemd|120"],
        ["openrc", openrc, "https://monitor.example.com|token-openrc|45"],
      ] as const) {
        const result = spawnSync("sh", ["-c", `
          set -eu
          ${functions}
          init_system=${init}
          SERVICE_FILE=$SERVICE_CONFIG
          OPENRC_FILE=$SERVICE_CONFIG
          load_installed_agent_config
          printf '%s|%s|%s\\n' "$endpoint" "$token" "$interval"
        `], { env: { ...process.env, SERVICE_CONFIG: path }, encoding: "utf8" });
        expect(result.status).toBe(0);
        expect(result.stdout).toBe(`${expected}\n`);
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test("accepts only the optional download mirror", () => {
    expect(spawnSync("sh", ["-n"], { input: agentInstaller }).status).toBe(0);
    const update = agentInstaller.slice(agentInstaller.indexOf("update_agent()"), agentInstaller.indexOf("\nstatus_agent()"));
    const functions = agentShellFunctions("update_agent", "load_installed_agent_config", "fail", "usage");
    const accepted = spawnSync("sh", ["-c", `
      set -eu
      ${functions}
      id() { printf '0'; }
      detect_init_system() { printf systemd; }
      load_installed_agent_config() { :; }
      install_agent() { printf 'install:%s:%s:%s:%s\\n' "$2" "$4" "$6" "$8"; }
      ${update}
      endpoint=https://monitor.example.com
      token=token
      interval=120
      update_agent -m https://ghproxy.net
    `], { encoding: "utf8" });
    expect(accepted.status).toBe(0);
    expect(accepted.stdout).toBe("install:https://monitor.example.com:token:120:https://ghproxy.net\n");

    const rejected = spawnSync("sh", ["-c", `
      set -eu
      ${functions}
      id() { printf '0'; }
      ${update}
      update_agent --bad
    `], { encoding: "utf8" });
    expect(rejected.status).toBe(1);
  });
});

describe("agent installer service safety", () => {
  test("a failed stop of an existing service prevents replacement", () => {
    const stop = agentInstaller.slice(agentInstaller.indexOf("  rollback_agent=true\n"), agentInstaller.indexOf('  mv "$temporary" "$AGENT_FILE"'));
    expect(stop).toContain("systemctl stop");
    for (const init of ["systemd", "openrc"]) {
      const result = spawnSync("sh", ["-c", `
        set -eu
        ${agentShellFunctions("fail")}
        init_system=$TEST_INIT
        had_service=true
        SERVICE_NAME=nodeflare-agent
        systemctl() { return 1; }
        rc-service() { return 1; }
        ${stop}
        printf 'replaced'
      `], { env: { ...process.env, TEST_INIT: init }, encoding: "utf8" });
      expect(result.status).toBe(1);
      expect(result.stdout).toBe("");
      expect(result.stderr).toContain("未替换程序");
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

  for (const scenario of ["healthy", "exits", "restarts", "query-fails"]) {
    test(`checks continuous agent startup: ${scenario}`, () => {
      const result = spawnSync("sh", ["-c", `
        set -eu
        SERVICE_NAME=nodeflare-agent
        tick=0
        sleep() { tick=$((tick + 1)); }
        systemctl() {
          case "$1" in
            is-active) [ "$SCENARIO" != exits ] || [ "$tick" -lt 2 ] ;;
            show)
              [ "$SCENARIO" != query-fails ] || return 1
              if [ "$SCENARIO" = restarts ] && [ "$tick" -ge 2 ]; then printf '456'; else printf '123'; fi
              ;;
            *) return 99 ;;
          esac
        }
        ${agentShellFunctions("verify_agent_started")}
        if verify_agent_started; then printf '%s' "$tick"; else exit 1; fi
      `], { env: { ...process.env, SCENARIO: scenario }, encoding: "utf8" });
      expect(result.status).toBe(scenario === "healthy" ? 0 : 1);
      if (scenario === "healthy") expect(result.stdout).toBe("10");
    });
  }

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
