import { describe, expect, it } from "vitest";
import {
  isAdditiveAppId,
  isProxyAppId,
  isStackAppId,
  usesEditorView,
} from "@/config/appConfig";

describe("appConfig provider lifecycle", () => {
  it.each(["opencode", "openclaw", "hermes", "pi", "mcode", "kimicode"])(
    "classifies %s as additive",
    (appId) => {
      expect(isAdditiveAppId(appId)).toBe(true);
    },
  );

  it.each(["claude", "claude-desktop", "codex", "gemini", "grokbuild"])(
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

  it.each(["claude-desktop", "opencode", "openclaw", "hermes", "pi", "mcode"])(
    "does not classify %s as proxy-capable",
    (appId) => {
      expect(isProxyAppId(appId)).toBe(false);
    },
  );

  it.each(["claude", "codex", "kimicode"])(
    "classifies %s as stack-capable",
    (appId) => {
      expect(isStackAppId(appId)).toBe(true);
    },
  );

  it.each(["gemini", "grokbuild", "mcode", "pi"])(
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
