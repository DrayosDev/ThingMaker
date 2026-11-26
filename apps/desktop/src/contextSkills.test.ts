import { expect, it, vi } from "vitest";
import type { SkillEntry } from "@thingmaker/contracts";

vi.mock("./ipc", () => ({ api: {} }));
import { agentListedSkills } from "./components/ContextPane";

const skill = (scope: SkillEntry["scope"], directoryName: string): SkillEntry =>
  ({ directoryName, scope, path: `/${scope}/${directoryName}/SKILL.md`, bytes: 1, contentHash: "h", resources: [], problems: [], shadowed: false });

it("does not say an agent can list a skill kept in ThingMaker's own folder", () => {
  const found = agentListedSkills([skill("project", "review"), skill("user", "notes"), skill("thingmaker", "big-thing")]);
  expect(found.map((entry) => entry.directoryName)).toEqual(["review", "notes"]);
});
