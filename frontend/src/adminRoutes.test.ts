import { describe, expect, test } from "bun:test";
import { adminTabFromPath, adminTabPaths, type AdminTab } from "./adminRoutes";

describe("admin routes", () => {
  test("maps every sidebar entry to its own stable URL", () => {
    for (const [tab, path] of Object.entries(adminTabPaths) as Array<[AdminTab, string]>) {
      expect(adminTabFromPath(path)).toBe(tab);
      expect(adminTabFromPath(`${path}/`)).toBe(tab);
    }
  });

  test("uses the server page for the admin root and unknown paths", () => {
    expect(adminTabFromPath("/admin")).toBe("servers");
    expect(adminTabFromPath("/admin/not-found")).toBe("servers");
  });
});
