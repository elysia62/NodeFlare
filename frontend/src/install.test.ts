import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const installer = readFileSync(new URL("../../install.sh", import.meta.url), "utf8");
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
