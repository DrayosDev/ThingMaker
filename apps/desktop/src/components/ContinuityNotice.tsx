import { useRef, useState } from "react";
import { t } from "../i18n";
import { IS_REMOTE } from "../remote/mode";
import { sessionContinuation, useStore } from "../store";

export function ContinuityNotice({ sessionId }: { sessionId: string }) {
  const archive = useStore((s) => {
    return sessionContinuation(s.sessions[sessionId], s.continuations)?.archive;
  });
  const exportContinuity = useStore((s) => s.exportContinuity);
  const [busy, setBusy] = useState(false);
  const locked = useRef(false);
  if (!archive) return null;
  const save = async () => {
    if (locked.current) return;
    locked.current = true;
    setBusy(true);
    try { await exportContinuity(sessionId); } catch { /* The store reports errors. */ } finally { locked.current = false; setBusy(false); }
  };
  return (
    <section aria-label={t("Continuity archive")} className="banner">
      <strong>{t("Private continuity archive available")}</strong>
      <p className="small">{t("The new leader can consult the earlier visible conversation a page at a time. Hidden reasoning, binary files and unsent drafts are not in it.")}</p>
      {/* A path on the computer, and the export, mean nothing on a phone. */}
      {!IS_REMOTE && <details><summary>{t("Archive location")}</summary><span className="small mono">{archive.path}</span></details>}
      {!IS_REMOTE && <button className="button button-small" disabled={busy} onClick={() => void save()} type="button">{busy ? t("Exporting…") : t("Export continuity…")}</button>}
    </section>
  );
}
