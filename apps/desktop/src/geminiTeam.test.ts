/**
 * A Gemini session leads a team only on native ACP, which it is opened on
 * when it starts with a team. One opened without a team cannot be given one
 * while it runs: the team is kept for it, and the next message continues in
 * a new Gemini session that leads it.
 */
import { beforeEach, expect, it, vi } from "vitest";
import type { OpenResponse, Snapshot } from "@thingmaker/contracts";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage: unknown = null; } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
import { api } from "./ipc";
import { useStore, switchesSession, EMPTY_SESSION_USAGE, type LiveSession } from "./store";
import { emptyProjection, EMPTY_QUEUE } from "./projection";

const initial = useStore.getState();
const team = { workers: [{ name: "Reviewer", provider: "claude" as const, capabilities: ["review"] }], nativeSubagents: false };
const solo = { workers: [], nativeSubagents: false };
function snapshot(id: string, leadsTeam: boolean): Snapshot {
  return { handle: { id, attachmentGeneration: "1" }, provider: "gemini", accountId: "default", leadsTeam, agentSessionId: `agent-${id}`, process: "ready", attachment: "attached", foreground: "idle", autonomousTurns: [], detachedCalls: {}, toolCalls: {}, backgroundTasks: [], configOptions: [{ id: "model", currentValue: "gemini-3.1-pro-high" }], capabilities: null, availableCommands: [], pendingSteers: [], queue: EMPTY_QUEUE, lastSequence: "0", historyStart: "0", historyDropped: 0, exit: null };
}
function live(id: string, leadsTeam: boolean): LiveSession {
  const projection = emptyProjection();
  Object.assign(projection, { process: "ready", attachment: "attached", foreground: "idle" });
  return { handle: { id, attachmentGeneration: "1" }, workspaceId: "workspace", snapshot: snapshot(id, leadsTeam), projection, inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
}

beforeEach(() => {
  vi.restoreAllMocks();
  useStore.setState({ ...initial, sessions: { plain: live("plain", false), lead: live("lead", true) }, sessionOrder: ["plain", "lead"], view: { kind: "session", sessionId: "plain" }, drafts: {}, attachments: {}, mentions: {}, teams: {}, jobs: {}, odyssey: {}, pendingProvider: {}, sideBrief: {}, continuations: {}, continuationHistory: {}, error: null,
    loadTeam: vi.fn(async () => undefined), loadOdyssey: vi.fn(async () => undefined), loadNesting: vi.fn(async () => undefined), renameSession: vi.fn(async () => undefined), archiveSession: vi.fn(async () => undefined),
  } as never);
  vi.spyOn(api, "delegationComboSet").mockImplementation(async (_handle, combo) => combo);
  vi.spyOn(api, "sessionSnapshot").mockImplementation(async (handle) => useStore.getState().sessions[handle.id]?.snapshot ?? snapshot(handle.id, true));
  vi.spyOn(api, "delegationJobs").mockResolvedValue([]);
  vi.spyOn(api, "odysseyForSession").mockResolvedValue(null);
  vi.spyOn(api, "sessionOpen").mockImplementation(async (): Promise<OpenResponse> => ({ handle: { id: "next", attachmentGeneration: "1" }, snapshot: snapshot("next", true), desktopSessionId: "row-next" }));
  vi.spyOn(api, "sessionSubscribe").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionHistory").mockResolvedValue([]);
  vi.spyOn(api, "prefSet").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionStop").mockResolvedValue({ status: 0, signal: null, forced: false });
  vi.spyOn(api, "sessionSubmit").mockResolvedValue({ outcome: { outcome: "accepted" } } as never);
});

it("moves a Gemini session without the team tools to a new one only when there are helpers to lead", () => {
  const plain = useStore.getState().sessions.plain!;
  const lead = useStore.getState().sessions.lead!;
  expect(switchesSession(plain, { provider: "gemini" }, team)).toBe(true);
  expect(switchesSession(plain, { provider: "gemini" }, solo)).toBe(false);
  expect(switchesSession(plain, { provider: "gemini" })).toBe(false);
  expect(switchesSession(lead, { provider: "gemini" }, team)).toBe(false);
  // A host from before the snapshot said so is taken to have the tools.
  expect(switchesSession({ snapshot: { provider: "gemini" } }, { provider: "gemini" }, team)).toBe(false);
  expect(switchesSession(plain, { provider: "claude" }, solo)).toBe(true);
});

it("gives a team to a Gemini session that has the tools while it runs", async () => {
  await useStore.getState().saveTeam("lead", team);
  expect(api.delegationComboSet).toHaveBeenCalledWith({ id: "lead", attachmentGeneration: "1" }, team);
  expect(useStore.getState().pendingProvider.lead).toBeUndefined();
});

it("keeps the team for a plain Gemini session and continues in a new one that leads it", async () => {
  await useStore.getState().saveTeam("plain", team);
  // Nothing is asked of the running session, which has no team to change.
  expect(api.delegationComboSet).not.toHaveBeenCalled();
  expect(useStore.getState().error).toBeNull();
  expect(useStore.getState().teams.plain).toEqual(team);
  expect(useStore.getState().pendingProvider.plain).toEqual({ provider: "gemini", model: "gemini-3.1-pro-high" });

  useStore.setState({ drafts: { plain: "Split this between you" } });
  await useStore.getState().send("plain");
  expect(api.sessionOpen).toHaveBeenCalledWith(expect.objectContaining({ provider: "gemini", mode: { mode: "new" }, model: "gemini-3.1-pro-high", combo: team }));
  expect(api.sessionSubmit).toHaveBeenCalledWith({ id: "next", attachmentGeneration: "1" }, expect.any(String), "Split this between you", [], []);
  expect(useStore.getState().pendingProvider.plain).toBeUndefined();
});

it("goes back to answering in place once the team is emptied again", async () => {
  await useStore.getState().saveTeam("plain", team);
  await useStore.getState().saveTeam("plain", solo);
  expect(useStore.getState().pendingProvider.plain).toBeUndefined();
  useStore.setState({ drafts: { plain: "Just you, then" } });
  await useStore.getState().send("plain");
  expect(api.sessionOpen).not.toHaveBeenCalled();
  expect(api.sessionSubmit).toHaveBeenCalledWith({ id: "plain", attachmentGeneration: "1" }, expect.any(String), "Just you, then", [], []);
});
