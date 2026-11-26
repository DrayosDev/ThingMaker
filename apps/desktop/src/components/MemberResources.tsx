import { useEffect, useState } from "react";
import { PROVIDER_LABELS, localAgentHarness, localModelId, parseLocalModel } from "@thingmaker/contracts";
import type { MemberResourceSelection, WorkerSlot } from "@thingmaker/contracts";
import { api, type ImportedSkillsProvider, type ImportedSkillsStatus, type ResourceCatalog } from "../ipc";
import { impliedSkills, resourceGroupLabel } from "../memberResourceDependencies";
import { useStore } from "../store";
import { t } from "../i18n";
import { SkillConnections } from "./SkillConnections";
import { Segmented } from "./settings/controls";

const errorText = (failure: unknown) => typeof failure === "object" && failure !== null && "message" in failure ? String(failure.message) : String(failure);
const IMPORT_PROVIDERS: ImportedSkillsProvider[] = ["claude", "codex", "gemini"];

/** First import of a provider's skills into ThingMaker; later catalog loads re-sync them. */
function ImportedSkillsSetup({ imports, onDone }: { imports: ImportedSkillsStatus[]; onDone: () => void }) {
  const [busy, setBusy] = useState<ImportedSkillsProvider | null>(null);
  const [failure, setFailure] = useState<{ provider: ImportedSkillsProvider; message: string } | null>(null);
  const setup = (provider: ImportedSkillsProvider) => {
    setBusy(provider);
    setFailure(null);
    void api.importedSkillsSetup(provider).then(
      (status) => { if (status.problem) setFailure({ provider, message: status.problem }); onDone(); },
      (error: unknown) => setFailure({ provider, message: errorText(error) }),
    ).finally(() => setBusy(null));
  };
  return <section className="imported-skills" aria-label={t("Imported skills")}>
    <p className="small muted">{t("Import a provider's skills once to select ThingMaker's copies here instead of the provider's own. They re-sync each time this list opens.")}</p>
    {IMPORT_PROVIDERS.map((provider) => {
      const status = imports.find((entry) => entry.provider === provider);
      const problem = failure?.provider === provider ? failure.message : status?.problem;
      return <div className="row wrap" key={provider} role="group" aria-label={PROVIDER_LABELS[provider]}>
        <strong>{PROVIDER_LABELS[provider]}</strong>
        <span className="small muted">{status?.imported
          ? t("{count} skills imported · synced {time}", { count: status.skills, time: status.syncedAt ? new Date(status.syncedAt).toLocaleString() : t("never") })
          : t("Not set up")}</span>
        <button className="button button-small" disabled={busy !== null} type="button" onClick={() => setup(provider)}>{busy === provider ? t("Importing…") : status?.imported ? t("Re-import") : t("Set up")}</button>
        {problem && <span className="small" role="alert">{problem}</span>}
      </div>;
    })}
  </section>;
}

/** The workspace is a preview: saved references resolve against the actual
 * job workspace at launch. Selecting a group is a snapshot, never a wildcard. */
export function MemberResources({ worker, onChange, initialWorkspace = "" }: { initialWorkspace?: string; worker: WorkerSlot; onChange: (worker: WorkerSlot) => void }) {
  const workspaces = useStore((s) => s.workspaces);
  const account = useStore((s) => `${s.accounts.claude?.find((entry) => entry.active)?.id ?? ""}:${s.accounts.codex?.find((entry) => entry.active)?.id ?? ""}`);
  const [workspaceId, setWorkspaceId] = useState(initialWorkspace);
  const [refresh, setRefresh] = useState(0);
  const [catalog, setCatalog] = useState<ResourceCatalog | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [tab, setTab] = useState<"skills" | "mcps">("skills");
  const localHarness = worker.provider === "local" ? localAgentHarness(worker.model) : undefined;
  const models = useStore((s) => s.providerModels.local);
  const loadModels = useStore((s) => s.loadProviderModels);
  useEffect(() => {
    if (localHarness && !models) void loadModels("local");
  }, [localHarness, models, loadModels]);
  const harness = localHarness ?? worker.provider;
  const supported = harness === "claude" || harness === "codex";
  const custom = worker.resources !== undefined;

  useEffect(() => {
    if (!supported || !custom) return;
    let cancelled = false;
    setCatalog(null);
    setError(null);
    const preview = localHarness ? api.memberResources(workspaceId || null, harness, true) : api.memberResources(workspaceId || null, harness);
    void preview.then(
      (value) => { if (!cancelled) setCatalog(value); },
      (failure: unknown) => { if (!cancelled) setError(errorText(failure)); },
    );
    return () => { cancelled = true; };
  }, [workspaceId, refresh, supported, custom, account, harness, localHarness]);

  if (!supported && !custom) return null;
  const inherit = () => {
    const { resources: _resources, ...rest } = worker;
    onChange(rest);
  };
  const selection = worker.resources ?? { skills: [], mcps: [] };
  const patch = (next: MemberResourceSelection) => onChange({ ...worker, resources: next });
  const kind = tab === "skills" ? "skill" : "mcp";
  const visible = (catalog?.entries ?? []).filter((entry) => entry.kind === kind && `${entry.name} ${entry.description} ${entry.group} ${entry.source ?? ""} ${entry.target ?? ""}`.toLowerCase().includes(search.toLowerCase()));
  const groups = [...new Set(visible.map((entry) => entry.group))].sort();
  // Dependencies are derived from the explicit picks on every render and never stored.
  const implied = impliedSkills(catalog?.entries ?? [], selection.skills);
  const effective = { ...selection, skills: [...selection.skills, ...implied.keys()] };
  const names = new Map(catalog?.entries.map((entry) => [entry.id, entry.name]));
  const nameList = (ids: string[]) => ids.map((id) => names.get(id) ?? id).join(", ");
  const known = new Set(catalog?.entries.map((entry) => entry.id));
  const missing = catalog ? [...selection.skills, ...selection.mcps].filter((id) => !known.has(id)) : [];

  return (
    <section className="member-resources" aria-label={t("Skills and connections")}>
      <div className="row wrap">
        {localHarness && <label>{t("Harness")} <select className="input" value={localHarness} onChange={(event) => onChange({
          ...worker, model: localModelId("agent", parseLocalModel(worker.model).name, event.target.value === "codex" ? "codex" : "claude"),
        })}>
          {(["claude", "codex"] as const).filter((id) => id === localHarness ||
            models?.some((model) => model.id === localModelId("agent", parseLocalModel(worker.model).name, id)))
            .map((id) => <option key={id} value={id}>{id === "codex" ? "Codex" : "Claude Code"}</option>)}
        </select></label>}
        <strong>{t("Skills and connections")}</strong>
        <select className="input" aria-label={t("Resource mode")} value={custom ? "custom" : "inherit"} onChange={(event) => event.target.value === "inherit" ? inherit() : patch({ skills: [], mcps: [] })}>
          <option value="inherit">{t("Inherit configuration")}</option>
          <option value="custom" disabled={!supported}>{t("Custom selection")}</option>
        </select>
      </div>
      {!supported && <p role="alert">{t("This selection requires Claude Code or Codex. Restore inherited configuration to use another provider.")}</p>}
      {custom && supported && <>
        <p className="small muted">{t("Only checked resources are available to this member. An empty selection enables none. The internal team connection stays available.")}</p>
        <label className="field">{t("Preview project")}
          <select className="input" value={workspaceId} onChange={(event) => setWorkspaceId(event.target.value)}>
            <option value="">{t("User resources only")}</option>
            {workspaces.map((workspace) => <option key={workspace.id} value={workspace.id}>{workspace.displayPath || workspace.canonicalRoot}</option>)}
          </select>
        </label>
        <div className="row wrap">
          <Segmented label={t("Resource type")} value={tab} onChange={setTab} choices={[{ value: "skills", label: `${t("Skills")} · ${effective.skills.length}` }, { value: "mcps", label: `MCPs · ${selection.mcps.length}` }]} />
          <button className="button button-small" onClick={() => setRefresh((value) => value + 1)} type="button">{t("Refresh")}</button>
          <input className="input" aria-label={t("Search resources")} placeholder={t("Search resources")} value={search} onChange={(event) => setSearch(event.target.value)} />
        </div>
        {error && <p role="alert">{error}</p>}
        {catalog?.problem && <p role="alert">{catalog.problem}</p>}
        {catalog && tab === "skills" && catalog.imports && <ImportedSkillsSetup imports={catalog.imports} onDone={() => setRefresh((value) => value + 1)} />}
        {catalog && <SkillConnections catalog={catalog} selection={effective} onChange={(next) => patch({ ...selection, mcps: next.mcps })} />}
        {!catalog && !error && <p className="small muted">{t("Loading resources…")}</p>}
        {missing.length > 0 && <div role="alert"><p>{t("Some selected resources are missing from this preview. They must be available in the task's project before this member can start.")}</p>{missing.map((id) => <label className="member-resource" key={id}><input type="checkbox" checked onChange={() => patch({ skills: selection.skills.filter((entry) => entry !== id), mcps: selection.mcps.filter((entry) => entry !== id) })} />{id}</label>)}</div>}
        <div className="member-resource-list">
          {groups.map((group) => {
            const entries = visible.filter((entry) => entry.group === group);
            const available = entries.filter((entry) => entry.available).map((entry) => entry.id);
            const all = available.length > 0 && available.every((id) => selection[tab].includes(id));
            return <details key={group} open>
              <summary>{resourceGroupLabel(group)} · {entries.length}</summary>
              <button className="button button-small" disabled={available.length === 0} type="button" onClick={() => patch({ ...selection, [tab]: all ? selection[tab].filter((id) => !available.includes(id)) : [...new Set([...selection[tab], ...available])] })}>{all ? t("Deselect group") : t("Select group")}</button>
              {entries.map((entry) => {
                const explicit = selection[tab].includes(entry.id);
                const includedBy = explicit || tab !== "skills" ? undefined : implied.get(entry.id);
                const dependencies = entry.skillDependencies ?? [];
                return <label className={`member-resource ${entry.available ? "" : "muted"}`} key={entry.id}>
                  <input type="checkbox" aria-label={entry.name} checked={explicit || includedBy !== undefined} disabled={includedBy !== undefined || (!entry.available && !explicit)} onChange={(event) => patch({ ...selection, [tab]: event.target.checked ? [...selection[tab], entry.id] : selection[tab].filter((id) => id !== entry.id) })} />
                  <span><strong>{entry.name}</strong>{entry.source && <span className="small muted">{entry.source}</span>}{entry.target && <span className="small muted">{entry.target}</span>}<span className="small muted">{entry.problem ?? entry.description}</span>
                    {entry.duplicateOf && <span className="small muted">{t("Same skill as ThingMaker's own copy")}</span>}
                    {dependencies.length > 0 && <span className="small muted">{t("Uses {skills}", { skills: nameList(dependencies) })}</span>}
                    {includedBy && <span className="small muted">{t("Included by {skills}", { skills: nameList(includedBy) })} <button className="link" type="button" onClick={() => patch({ ...selection, skills: [...selection.skills, entry.id] })}>{t("Keep selected")}</button></span>}
                  </span>
                </label>;
              })}
            </details>;
          })}
          {catalog && visible.length === 0 && <p className="small muted">{t("No matching resources.")}</p>}
        </div>
        <p className="small muted">{t("Changes apply to new tasks. Files, terminal access and plugin hooks are controlled separately.")}</p>
        <details><summary className="small muted">{t("Pilot scope")}</summary>{catalog?.notes.map((note) => <p className="small muted" key={note}>{note}</p>)}</details>
      </>}
    </section>
  );
}
