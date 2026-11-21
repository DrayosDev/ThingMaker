import { useState } from "react";
import { t } from "../i18n";
import { sessionTitle, useStore } from "../store";
import { Select, SettingGroup, SettingRow, Switch } from "./settings/controls";

export function ContinuitySettings() {
  const sessions = useStore((s) => s.sessions);
  const order = useStore((s) => s.sessionOrder);
  const history = useStore((s) => s.navHistory);
  const view = useStore((s) => s.view);
  const uiPrefs = useStore((s) => s.uiPrefs);
  const setUiPrefs = useStore((s) => s.setUiPrefs);
  const exportContinuity = useStore((s) => s.exportContinuity);
  const [selected, setSelected] = useState("");
  const [busy, setBusy] = useState(false);
  const ids = [...order.filter((id) => sessions[id]), ...Object.keys(sessions).filter((id) => !order.includes(id))];
  const previous = view.kind === "session" ? view : [...history].reverse().find((entry) => entry.view.kind === "session" && sessions[entry.view.sessionId])?.view;
  const preferred = previous?.kind === "session" ? previous.sessionId : "";
  const current = sessions[selected] ? selected : sessions[preferred] ? preferred : ids[0] ?? "";
  const save = async () => {
    if (!current || busy) return;
    setBusy(true);
    try { await exportContinuity(current); } catch { /* The store reports errors. */ } finally { setBusy(false); }
  };
  return (
    <>
      <SettingGroup note={t("Private Markdown files stay in ThingMaker app data. Export lets you choose where to save a copy.")}>
        <SettingRow name={t("Automatic continuity")} hint={t("Save private Markdown before changing leader or provider, including quota continuation. Automatic continuity needs an agent with tools; turn it off to continue in Local Chat with a limited excerpt.")}>
          <Switch checked={uiPrefs.autoContinuity !== false} label={t("Automatic continuity")} onChange={(on) => void setUiPrefs({ autoContinuity: on })} />
        </SettingRow>
        <SettingRow name={t("What carries over")} hint={t("The next leader can consult the full available history in chunks. Hidden reasoning is not restored; binary image and audio contents and unsent drafts are excluded. Truncated or unavailable provider content is marked.")} />
      </SettingGroup>
      <SettingGroup title={t("Export continuity")}>
        <SettingRow name={t("Conversation")} hint={ids.length ? undefined : t("No open conversations")}>
          <Select choices={ids.map((id) => ({ value: id, label: sessionTitle(sessions[id]!) }))} disabled={!ids.length || busy} label={t("Conversation")} onChange={setSelected} value={current || null} placeholder={t("No open conversations")} />
          <button className="set-button" disabled={!current || busy} onClick={() => void save()} type="button">{busy ? t("Exporting…") : t("Export Markdown…")}</button>
        </SettingRow>
      </SettingGroup>
    </>
  );
}
