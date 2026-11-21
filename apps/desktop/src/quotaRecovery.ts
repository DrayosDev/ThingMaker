import type { AccountView, Provider, Snapshot } from "@thingmaker/contracts";

/** Whether provider-wide readings apply to the account pinned to this session. */
export function sessionUsesActiveAccount(snapshot: Pick<Snapshot, "provider" | "accountId">, accounts: Partial<Record<Provider, AccountView[]>>): boolean {
  return !snapshot.accountId || snapshot.accountId === (accounts[snapshot.provider]?.find((account) => account.active)?.id ?? "default");
}

/** Recognise explicit exhaustion, not generic throttling or transport errors. */
export function isQuotaExhaustion(message: string | null | undefined): boolean {
  if (!message) return false;
  return /usage[_ ]?limit[_ ]?exceeded|insufficient[_ ]quota|(?:quota|credits?|usage limit).{0,40}(?:exhausted|exceeded|reached|depleted|spent|used up)|(?:out of|exceeded|reached|hit your).{0,40}(?:quota|credits?|usage limit)|you(?:['’]ve| have) hit your limit/i.test(message);
}
