import { useEffect, useRef, useState } from "react";
import { ACCOUNT_PROVIDERS, PROVIDER_LABELS, type Provider } from "@thingmaker/contracts";
import { useStore } from "../store";
import { t } from "../i18n";
import { accountName } from "./ProviderAccounts";

/** An explicit escape from an exhausted subscription, scoped to this session. */
export function QuotaRecovery({ sessionId }: { sessionId: string }) {
  const session = useStore((s) => s.sessions[sessionId]);
  const accounts = useStore((s) => s.accounts);
  const autoContinuity = useStore((s) => s.uiPrefs.autoContinuity !== false);
  const providers = useStore((s) => s.providers);
  const loadAccounts = useStore((s) => s.loadAccounts);
  const prepare = useStore((s) => s.prepareQuotaContinuation);
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState("");
  const [working, setWorking] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const locked = useRef(false);
  const exhausted = session?.projection.quotaFailure;

  useEffect(() => {
    setOpen(false);
    setSelected("");
    setProblem(null);
  }, [sessionId]);

  useEffect(() => {
    if (!open || !exhausted) return;
    for (const provider of ACCOUNT_PROVIDERS) void loadAccounts(provider);
  }, [open, exhausted, loadAccounts]);

  if (!session || !exhausted) return null;
  const source = session.snapshot.provider;
  const active = ["running", "awaiting_user", "cancelling"].includes(session.projection.foreground) || Boolean(session.inFlightRequestId);
  const targets: Array<{ key: string; provider: Provider; accountId?: string; label: string; disabled: boolean }> = [];
  for (const provider of ACCOUNT_PROVIDERS) {
    const info = providers?.find((entry) => entry.provider === provider);
    const installed = Boolean(info?.resolved);
    const list = accounts[provider];
    if (list?.length) {
      for (const account of list) {
        if (provider === source && (session.snapshot.accountId ? account.id === session.snapshot.accountId : account.active)) continue;
        targets.push({
          key: JSON.stringify([provider, account.id]), provider, accountId: account.id,
          label: `${PROVIDER_LABELS[provider]} · ${accountName(account)}${account.auth?.account ? ` · ${account.auth.account}` : ""}`,
          disabled: !installed || account.auth?.loggedIn !== true || Boolean(account.auth.setupRequired),
        });
      }
    } else if (provider !== source) {
      targets.push({ key: provider, provider, label: PROVIDER_LABELS[provider], disabled: !installed || info?.auth?.loggedIn !== true || Boolean(info?.auth?.setupRequired) });
    }
  }
  const target = targets.find((candidate) => candidate.key === selected && !candidate.disabled);
  const choose = async () => {
    if (!target || locked.current || active) return;
    locked.current = true;
    setWorking(true);
    setProblem(null);
    try {
      const key = await prepare(sessionId, { provider: target.provider, ...(target.accountId ? { accountId: target.accountId } : {}) });
      if (!key) setProblem(useStore.getState().error?.message ?? t("The continuation could not be prepared. Your original conversation is still available."));
    } catch {
      setProblem(t("The continuation could not be prepared. Your original conversation is still available."));
    } finally {
      locked.current = false;
      setWorking(false);
    }
  };
  return (
    <section aria-label={t("Continue after a usage limit")} className="banner quota-recovery">
      <strong>{t("{provider} reported that this session's usage limit was reached.", { provider: PROVIDER_LABELS[source] })}</strong>
      <p className="small">{t("You can wait for the limit to reset, or prepare a continuation with another signed-in account or provider.")}</p>
      <button aria-expanded={open} className="button button-small" disabled={working} onClick={() => setOpen(!open)} type="button">
        {t("Continue with another account or provider")}
      </button>
      {open && (
        <div className="quota-recovery-options">
          <label className="field">
            {t("Continue with")}
            <select aria-label={t("Continue with")} className="select" disabled={working} onChange={(event) => setSelected(event.target.value)} value={selected}>
              <option value="">{t("Choose an account or provider")}</option>
              {targets.map((entry) => <option disabled={entry.disabled} key={entry.key} value={entry.key}>{entry.label}{entry.disabled ? ` · ${t("not available")}` : ""}</option>)}
            </select>
          </label>
          {!targets.some((entry) => !entry.disabled) && <p className="small">{t("Sign in to another account in Providers to continue here.")}</p>}
          <p className="small muted">{autoContinuity ? t("The new session receives a recent excerpt and can read the private archive of available history. Your draft stays editable; nothing is sent until you send it.") : t("The new session receives a limited summary of the conversation. Your draft stays editable; nothing is sent until you send it.")}</p>
          {active && <p className="small">{t("Wait for the running turn to finish.")}</p>}
          <button className="button button-primary button-small" disabled={!target || working || active} onClick={() => void choose()} type="button">
            {working ? t("Preparing continuation…") : t("Prepare continuation")}
          </button>
          {problem && <p className="small" role="alert">{problem}</p>}
        </div>
      )}
    </section>
  );
}
