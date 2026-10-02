import { describe, expect, test } from "bun:test";
import { installCommandFor } from "./InstallDialog";
import { emptyServer, toInput, trafficTimezones, type AgentInstallInfo } from "./shared";
import type { AdminServer } from "../../types";

const install: AgentInstallInfo = {
  agent_token: "secret-token",
  agent_mirror: "",
};

describe("Agent install command builder", () => {
  test("uses each platform's native downloader", () => {
    expect(installCommandFor(install, "linux", "https://panel.example.com"))
      .toContain("curl -fsSL");
    expect(installCommandFor(install, "linux", "https://panel.example.com"))
      .toContain("agent.sh");
    expect(installCommandFor(install, "freebsd", "https://panel.example.com"))
      .toContain("fetch -qo -");
    expect(installCommandFor(install, "freebsd", "https://panel.example.com"))
      .toContain("install-freebsd.sh");
    expect(installCommandFor(install, "macos", "https://panel.example.com"))
      .toContain("install-macos.sh");
    expect(installCommandFor(install, "windows", "https://panel.example.com"))
      .toContain("Invoke-WebRequest");
  });

  test("quotes the endpoint and token for the target shell", () => {
    const linux = installCommandFor(install, "linux", "https://panel.example.com");
    expect(linux).toContain("-e 'https://panel.example.com'");
    expect(linux).toContain("-t 'secret-token'");

    // PowerShell arguments are single-quoted too; the surrounding
    // double quotes belong to the download URLs.
    const windows = installCommandFor(install, "windows", "https://panel.example.com");
    expect(windows).toContain("-e 'https://panel.example.com'");
    expect(windows).toContain("-t 'secret-token'");
  });

  test("appends the mirror with the platform's own flag", () => {
    const mirrored = { ...install, agent_mirror: "https://ghproxy.net/" };
    // The trailing slash is trimmed before being embedded.
    expect(installCommandFor(mirrored, "linux", "https://panel.example.com"))
      .toContain("-m 'https://ghproxy.net'");
    expect(installCommandFor(mirrored, "windows", "https://panel.example.com"))
      .toContain("-Mirror 'https://ghproxy.net'");
  });

  test("omits the mirror flag entirely when none is configured", () => {
    const linux = installCommandFor(install, "linux", "https://panel.example.com");
    expect(linux).not.toContain(" -m ");
    expect(installCommandFor(install, "windows", "https://panel.example.com"))
      .not.toContain("-Mirror");
    // A mirror must never leave a dangling flag when it is empty.
    expect(linux.trimEnd()).toMatch(/secret-token' --disable-remote$/);
  });

  test("escapes quotes inside the token so the command stays valid", () => {
    const tricky = { ...install, agent_token: `tok'en` };
    // The shell literal must escape the embedded quote rather than break out.
    expect(installCommandFor(tricky, "linux", "https://panel.example.com"))
      .toContain(`'tok'\\''en'`);
  });

  test("new install commands disable remote control unless explicitly allowed", () => {
    for (const platform of ["linux", "macos", "freebsd", "windows"] as const) {
      const flag = platform === "windows" ? "-DisableRemote" : "--disable-remote";
      expect(installCommandFor(install, platform, "https://panel.example.com")).toEndWith(flag);
      const disabled = installCommandFor(install, platform, "https://panel.example.com", false);
      expect(disabled).toEndWith(flag);
      const enabled = installCommandFor(install, platform, "https://panel.example.com", true);
      expect(enabled).not.toContain(flag);
      expect(enabled).toEndWith("-t 'secret-token'");
    }
  });
});

describe("node traffic timezone", () => {
  test("defaults to UTC and preserves the configured timezone when editing", () => {
    expect(emptyServer.reset_timezone).toBe("UTC");
    const server = { ...emptyServer, reset_timezone: "America/Los_Angeles" } as AdminServer;
    expect(toInput(server).reset_timezone).toBe("America/Los_Angeles");
    for (const locale of ["zh-CN", "en-US"]) {
      for (const zone of trafficTimezones(locale)) {
        expect(() => new Intl.DateTimeFormat("en-US", { timeZone: zone.value })).not.toThrow();
      }
    }
  });
});
