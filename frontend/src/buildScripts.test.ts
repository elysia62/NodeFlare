import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import { spawnSync } from "node:child_process";

const buildScript = readFileSync(new URL("../../scripts/build-frontend.sh", import.meta.url), "utf8");
const versionScript = readFileSync(new URL("../../scripts/resolve-version.sh", import.meta.url), "utf8");

describe("frontend build dependency synchronization", () => {
  for (const existingModules of [false, true]) {
    for (const installFails of [false, true]) {
      test(`existing node_modules=${existingModules}, install fails=${installFails}`, () => {
        const directory = mkdtempSync(join(tmpdir(), "nodeflare-build-script-"));
        try {
          const scripts = join(directory, "scripts");
          const bin = join(directory, "bin");
          const log = join(directory, "commands.log");
          mkdirSync(scripts);
          mkdirSync(bin);
          if (existingModules) mkdirSync(join(directory, "node_modules"));
          writeFileSync(join(scripts, "build-frontend.sh"), buildScript, { mode: 0o755 });
          writeFileSync(join(scripts, "resolve-version.sh"), versionScript, { mode: 0o755 });
          writeFileSync(join(bin, "bun"), `#!/bin/sh
set -eu
printf '%s\\n' "$*" >> "$BUILD_TEST_LOG"
if [ "$1" = install ]; then
  [ "$2" = --frozen-lockfile ] || exit 24
  [ "$BUILD_TEST_FAIL" != true ] || exit 23
else
  [ "$VITE_NODEFLARE_VERSION" = 1.2.3 ] || exit 25
  [ "$PWD" = "$BUILD_TEST_ROOT" ] || exit 26
fi
`, { mode: 0o755 });
          const result = spawnSync("sh", [join(scripts, "build-frontend.sh")], {
            cwd: tmpdir(),
            env: {
              ...process.env,
              PATH: `${bin}${delimiter}${process.env.PATH ?? ""}`,
              NODEFLARE_VERSION: "v1.2.3",
              BUILD_TEST_ROOT: directory,
              BUILD_TEST_LOG: log,
              BUILD_TEST_FAIL: String(installFails),
            },
            encoding: "utf8",
          });
          expect(result.status).toBe(installFails ? 23 : 0);
          expect(result.stderr).toBe("");
          expect(readFileSync(log, "utf8")).toBe(installFails
            ? "install --frozen-lockfile\n"
            : "install --frozen-lockfile\nrun --cwd frontend build\n");
        } finally {
          rmSync(directory, { recursive: true, force: true });
        }
      });
    }
  }
});
