import { describe, expect, test } from "bun:test";
import { ADMIN_LOGIN_PATH, adminTabFromPath, adminTabPaths, canonicalAdminPath, type AdminTab } from "./adminRoutes";

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

  test("sends every unauthenticated admin route to the login page", () => {
    expect(canonicalAdminPath("/admin", false)).toBe(ADMIN_LOGIN_PATH);
    expect(canonicalAdminPath("/admin/servers", false)).toBe(ADMIN_LOGIN_PATH);
    expect(canonicalAdminPath("/admin/about", false)).toBe(ADMIN_LOGIN_PATH);
    expect(canonicalAdminPath("/admin/login/", false)).toBe(ADMIN_LOGIN_PATH);
  });

  test("sends an authenticated login page to servers and preserves valid pages", () => {
    expect(canonicalAdminPath("/admin/login", true)).toBe(adminTabPaths.servers);
    expect(canonicalAdminPath("/admin/login/", true)).toBe(adminTabPaths.servers);
    expect(canonicalAdminPath("/admin/about", true)).toBe(adminTabPaths.about);
    expect(canonicalAdminPath("/admin/not-found", true)).toBe(adminTabPaths.servers);
  });
});
