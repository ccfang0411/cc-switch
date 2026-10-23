import { describe, expect, it } from "vitest";
import {
  isAdditiveAppId,
  isProxyAppId,
  isStackAppId,
  usesEditorView,
  SKILLS_APP_IDS,
  MCP_APP_IDS,
} from "@/config/appConfig";

describe("appConfig provider lifecycle", () => {
  it.each(["opencode", "openclaw", "hermes", "pi", "mcode", "kimicode"])(
    "classifies %s as additive",
    (appId) => {
      expect(isAdditiveAppId(appId)).toBe(true);
    },
  );

  it.each(["claude", "claude-desktop", "codex", "gemini", "grokbuild", "agy"])(
    "does not classify %s as additive",
    (appId) => {
      expect(isAdditiveAppId(appId)).toBe(false);
    },
  );
});

describe("appConfig proxy capability", () => {
  it.each(["claude", "codex", "gemini", "grokbuild", "kimicode"])(
    "classifies %s as proxy-capable",
    (appId) => {
      expect(isProxyAppId(appId)).toBe(true);
    },
  );

  it.each([
    "claude-desktop",
    "opencode",
    "openclaw",
    "hermes",
    "pi",
    "mcode",
    "agy",
  ])("does not classify %s as proxy-capable", (appId) => {
    expect(isProxyAppId(appId)).toBe(false);
  });

  it.each(["claude", "codex", "kimicode"])(
    "classifies %s as stack-capable",
    (appId) => {
      expect(isStackAppId(appId)).toBe(true);
    },
  );

  it.each(["gemini", "grokbuild", "mcode", "pi", "agy"])(
    "does not classify %s as stack-capable",
    (appId) => {
      expect(isStackAppId(appId)).toBe(false);
    },
  );

  it("keeps kimicode both additive and proxy-capable (coexist in direct mode)", () => {
    expect(isAdditiveAppId("kimicode")).toBe(true);
    expect(isProxyAppId("kimicode")).toBe(true);
    expect(isStackAppId("kimicode")).toBe(true);
    expect(usesEditorView("kimicode")).toBe(true);
  });
});

// Antigravity shares management UI, but has independent configuration and no CCS proxy.
describe("Antigravity capabilities", () => {
  it("supports skills and MCP with direct provider switching", () => {
    expect(SKILLS_APP_IDS).toContain("agy");
    expect(MCP_APP_IDS).toContain("agy");
    expect(isAdditiveAppId("agy")).toBe(false);
    expect(isProxyAppId("agy")).toBe(false);
  });
});
