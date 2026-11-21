import { describe, expect, it } from "vitest";
import { isQuotaExhaustion, sessionUsesActiveAccount } from "./quotaRecovery";

describe("quota exhaustion", () => {
  it("does not reuse the active account's readings for a different pinned account", () => {
    const accounts = { codex: [{ id: "default", provider: "codex" as const, label: "Main", isDefault: true, active: true }] };
    expect(sessionUsesActiveAccount({ provider: "codex", accountId: "work" }, accounts)).toBe(false);
    expect(sessionUsesActiveAccount({ provider: "codex", accountId: "default" }, accounts)).toBe(true);
    expect(sessionUsesActiveAccount({ provider: "codex" }, accounts)).toBe(true);
  });
  it.each(["usageLimitExceeded", "insufficient_quota", "Your credits are exhausted", "out of quota", "usage limit reached", "You've hit your usage limit. Try again in 2 hours.", "You've hit your limit · resets 3pm"])("recognises %s", (message) => {
    expect(isQuotaExhaustion(message)).toBe(true);
  });
  it.each(["429 Too Many Requests", "rate limit", "network error", "quota information unavailable", "credits available"])("does not infer exhaustion from %s", (message) => {
    expect(isQuotaExhaustion(message)).toBe(false);
  });
});
