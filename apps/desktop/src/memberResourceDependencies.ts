import type { MemberMcpDependency, MemberResourceCatalog, MemberResourceEntry, MemberResourceSelection } from "@thingmaker/contracts";
import type { CatalogEntry } from "./ipc";
import { t } from "./i18n";

export type SkillConnection = MemberMcpDependency & { skills: string[]; candidates: MemberResourceEntry[] };

export function skillConnections(catalog: MemberResourceCatalog, selection: MemberResourceSelection): SkillConnection[] {
  const rows = new Map<string, SkillConnection>();
  for (const skill of catalog.entries.filter((entry) => entry.kind === "skill" && selection.skills.includes(entry.id))) {
    for (const dependency of skill.mcpDependencies ?? []) {
      const key = JSON.stringify([dependency.name.toLowerCase(), [...dependency.mcpIds].sort()]);
      const row = rows.get(key);
      if (row) {
        row.skills = [...new Set([...row.skills, skill.name])];
        row.required ||= dependency.required;
        if (dependency.evidence === "declared") row.evidence = "declared";
      } else rows.set(key, { ...dependency, skills: [skill.name], candidates: catalog.entries.filter((entry) => entry.kind === "mcp" && dependency.mcpIds.includes(entry.id)) });
    }
  }
  return [...rows.values()];
}

/** Different origins may define the same runtime server. Explicit activation
 * replaces that conflicting definition, while preserving unrelated connections. */
export function activateMcp(catalog: MemberResourceCatalog, selection: MemberResourceSelection, id: string): MemberResourceSelection {
  const target = catalog.entries.find((entry) => entry.id === id && entry.kind === "mcp" && entry.available);
  if (!target) return selection;
  const conflicts = new Set(catalog.entries.filter((entry) => entry.kind === "mcp" && entry.name === target.name && entry.id !== target.id).map((entry) => entry.id));
  return { ...selection, mcps: [...new Set([...selection.mcps.filter((id) => !conflicts.has(id)), target.id])] };
}

/** Batch activation never chooses among alternatives or displaces a selected
 * definition. Such conflicts require an explicit choice in the individual row. */
export function unambiguousMcps(rows: SkillConnection[], selection: MemberResourceSelection, catalog: MemberResourceCatalog): string[] {
  const targets = rows.filter((row) => !row.candidates.some((entry) => entry.available && selection.mcps.includes(entry.id)))
    .map((row) => row.candidates.filter((entry) => entry.available)).filter((entries) => entries.length === 1).map((entries) => entries[0]!);
  return [...new Set(targets.filter((target) => !targets.some((other) => other.id !== target.id && other.name === target.name)
    && !catalog.entries.some((entry) => entry.kind === "mcp" && entry.name === target.name && entry.id !== target.id && selection.mcps.includes(entry.id))).map((entry) => entry.id))];
}

/** How a catalog group reads in the selector. */
export function resourceGroupLabel(group: string): string {
  if (group === "builtin" || group === "thingmaker") return "ThingMaker";
  if (group === "user") return t("Local · user");
  if (group === "project") return t("Local · project");
  return `${t("Plugin")} · ${group.slice(7)}`;
}

/** What a selection that asks for referenced skills adds to the ticked ones,
 * the way the launch does it: every available skill reached from a ticked one
 * through `skillDependencies`, each mapped to the ticked skills it comes
 * from. A skill the person took out is `removed` instead, with the same
 * mapping, and nothing is reached through it. Ticked skills are in neither:
 * the stored selection holds what the person ticked, and these are derived
 * again on every change. A selection that does not ask adds nothing. */
export function referencedSkills(entries: CatalogEntry[], selection: MemberResourceSelection): { included: Map<string, string[]>; removed: Map<string, string[]> } {
  const included = new Map<string, string[]>();
  const removed = new Map<string, string[]>();
  if (!selection.referencedSkills) return { included, removed };
  const skills = new Map(entries.filter((entry) => entry.kind === "skill").map((entry) => [entry.id, entry]));
  const chosen = new Set(selection.skills);
  const excluded = new Set(selection.excludedSkills);
  for (const root of selection.skills) {
    const seen = new Set([root]);
    const pending = [...(skills.get(root)?.skillDependencies ?? [])];
    while (pending.length > 0) {
      const id = pending.pop()!;
      const entry = skills.get(id);
      if (seen.has(id) || !entry?.available) continue;
      seen.add(id);
      if (chosen.has(id)) continue;
      const side = excluded.has(id) ? removed : included;
      side.set(id, [...(side.get(id) ?? []), root]);
      if (side === included) pending.push(...(entry.skillDependencies ?? []));
    }
  }
  return { included, removed };
}

/** A selection as it is stored: the two fields about referenced skills are
 * left out when they say nothing. */
export function storedSelection(selection: MemberResourceSelection): MemberResourceSelection {
  const { referencedSkills: asked, excludedSkills: excluded, ...ticked } = selection;
  return { ...ticked, ...(asked ? { referencedSkills: true, ...(excluded?.length ? { excludedSkills: [...new Set(excluded)] } : {}) } : {}) };
}
