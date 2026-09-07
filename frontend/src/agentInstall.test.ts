import { describe, expect, test } from "bun:test";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

for (const platform of ["macos", "freebsd"] as const) {
  const script = readFileSync(new URL(`../../agent/install-${platform}.sh`, import.meta.url), "utf8");
  const functions = ["fail", "parse_agent_args", "load_installed_agent_config"].map((name) => {
    const match = script.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
    if (!match) throw new Error(`Missing ${platform} function: ${name}`);
    return match[0];
  }).join("\n");

  describe(`${platform} agent update`, () => {
    function parse(args: string[], config: "valid" | "missing" | "incomplete" = "valid") {
      const directory = mkdtempSync(join(tmpdir(), "nodeflare-agent-update-"));
      const file = join(directory, "service");
      try {
        if (config !== "missing") {
          writeFileSync(file, [
            'command_args="-P ${pidfile} -r -R 10 -S -T ${name} /usr/local/libexec/nodeflare/agent -e https://monitor.example.com -i 120"',
            config === "valid" ? 'export NODEFLARE_AGENT_TOKEN="existing-token"' : "",
          ].join("\n"));
        }
        return spawnSync("sh", ["-c", `
          set -eu
          ${functions}
          SERVICE_FILE=$TEST_SERVICE_FILE
          PLIST_FILE=$TEST_SERVICE_FILE
          AGENT_FILE=/usr/local/libexec/nodeflare/agent
          plist_value() {
            case "$1" in
              ProgramArguments:0) printf '%s' "$AGENT_FILE" ;;
              ProgramArguments:1) printf '%s' '-e' ;;
              ProgramArguments:2) printf '%s' 'https://monitor.example.com' ;;
              ProgramArguments:3) printf '%s' '-i' ;;
              ProgramArguments:4) printf '%s' '120' ;;
              EnvironmentVariables:NODEFLARE_AGENT_TOKEN)
                [ "$TEST_CONFIG" = valid ] || return 1
                printf '%s' 'existing-token' ;;
              *) return 1 ;;
            esac
          }
          parse_agent_args "$@"
          printf '%s|%s|%s|%s' "$endpoint" "$token" "$interval" "$mirror"
        `, "agent-test", ...args], {
          env: { ...process.env, TEST_SERVICE_FILE: file, TEST_CONFIG: config }, encoding: "utf8",
        });
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    }

    test("script syntax and documented update option", () => {
      expect(spawnSync("sh", ["-n"], { input: script }).status).toBe(0);
      expect(script).toContain(`install-${platform}.sh --update [-m`);
    });

    test("reuses installed credentials and interval with an optional mirror", () => {
      for (const args of [["--update"], ["--update", "-m", "https://mirror.example.com"]]) {
        const result = parse(args);
        expect(result.status).toBe(0);
        expect(result.stdout).toBe(`https://monitor.example.com|existing-token|120|${args[2] ?? ""}`);
      }
    });

    test("still parses a fresh installation", () => {
      const result = parse(["-e", "https://new.example.com", "-t", "new-token", "-i", "45"]);
      expect(result.status).toBe(0);
      expect(result.stdout).toBe("https://new.example.com|new-token|45|");
    });

    test("rejects missing or incomplete installed configuration", () => {
      for (const config of ["missing", "incomplete"] as const) {
        const result = parse(["--update"], config);
        expect(result.status).toBe(1);
        expect(result.stdout).toBe("");
        expect(result.stderr).not.toContain("existing-token");
      }
    });

    test("update cannot silently replace credentials or accept malformed options", () => {
      for (const args of [
        ["--update", "-e", "https://other.example.com"], ["--update", "-t", "new-token"],
        ["--update", "-i", "30"], ["--update", "--update"], ["--update", "--bad"],
        ["--update", "-m"], ["--update", "-m", ""],
        ["--update", "-m", "https://one.example.com", "-m", "https://two.example.com"],
      ]) {
        expect(parse(args).status).toBe(1);
      }
    });
  });
}
