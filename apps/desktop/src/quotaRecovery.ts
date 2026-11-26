import type { AccountView, Provider, Snapshot } from "@thingmaker/contracts";

/**
 * Whether provider-wide readings apply to the account this session runs on.
 * Until the provider's accounts have been read there is no telling which one
 * is active: the session is taken to be on it, as every session was before
 * one could be pinned, rather than its readings being dropped.
 */
export function sessionUsesActiveAccount(snapshot: Pick<Snapshot, "provider" | "accountId">, accounts: Partial<Record<Provider, AccountView[]>>): boolean {
  const known = accounts[snapshot.provider];
  if (!snapshot.accountId || !known) return true;
  return snapshot.accountId === (known.find((account) => account.active)?.id ?? "default");
}

/** Recognise explicit exhaustion, not generic throttling or transport errors. */
export function isQuotaExhaustion(message: string | null | undefined): boolean {
  if (!message) return false;
  return /usage[_ ]?limit[_ ]?exceeded|insufficient[_ ]quota|(?:quota|credits?|usage limit).{0,40}(?:exhausted|exceeded|reached|depleted|spent|used up)|(?:out of|exceeded|reached|hit your).{0,40}(?:quota|credits?|usage limit)|you(?:['’]ve| have) hit your limit/i.test(message);
}
