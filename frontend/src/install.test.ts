import { describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const installer = readFileSync(new URL("../../install.sh", import.meta.url), "utf8");
const startFunction = installer.slice(installer.indexOf("start_server() {"), installer.indexOf("\nuninstall_server() {"));

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
