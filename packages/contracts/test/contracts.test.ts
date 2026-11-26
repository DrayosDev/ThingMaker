import { describe, expect, it } from "vitest";
import { COMMANDS, PROVIDERS, PROVIDER_LABELS, isEffortOption } from "../src/index";

describe("contracts", () => {
  it("recognizes the advertised native Gemini reasoning option alongside existing adapters", () => {
    for (const id of ["effort", "reasoning_effort", "thought_level"]) expect(isEffortOption(id)).toBe(true);
    for (const id of ["model", "mode", "thought_level_extra"]) expect(isEffortOption(id)).toBe(false);
  });
  it("names every command once", () => {
    const names = Object.values(COMMANDS);
    expect(new Set(names).size).toBe(names.length);
    expect(names.every((name) => /^[a-z_]+$/.test(name))).toBe(true);
  });

  it("labels every provider", () => {
    expect(PROVIDERS).toEqual(["claude", "codex", "gemini", "local"]);
    for (const provider of PROVIDERS) expect(PROVIDER_LABELS[provider]).toBeTruthy();
  });
});
