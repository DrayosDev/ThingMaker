/**
 * From the phone a change of provider carries the recent excerpt only: the
 * continuity archives are written by the computer's own interface, and the
 * phone cannot call the commands that make or bind one.
 */
import { beforeEach, expect, it, vi } from "vitest";
import type { Snapshot } from "@thingmaker/contracts";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage: unknown = null; } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
vi.mock("./remote/mode", () => ({ IS_REMOTE: true }));
import { api } from "./ipc";
import { useStore, EMPTY_SESSION_USAGE, type LiveSession } from "./store";
import { emptyProjection, EMPTY_QUEUE } from "./projection";

const initial = useStore.getState();
function snapshot(id: string, provider: Snapshot["provider"]): Snapshot {
  return { handle: { id, attachmentGeneration: "1" }, provider, accountId: "default", leadsTeam: true, agentSessionId: `agent-${id}`, process: "ready", attachment: "attached", foreground: "idle", autonomousTurns: [], detachedCalls: {}, toolCalls: {}, backgroundTasks: [], configOptions: [], capabilities: null, availableCommands: [], pendingSteers: [], queue: EMPTY_QUEUE, lastSequence: "0", historyStart: "0", historyDropped: 0, exit: null };
}

beforeEach(() => {
  vi.restoreAllMocks();
  const projection = emptyProjection();
  Object.assign(projection, { process: "ready", attachment: "attached", foreground: "idle" });
  projection.cards.push({ kind: "message", key: "u", message: { key: "u", messageId: "u", role: "user", contentUnknown: false, blocks: [{ type: "text", text: "Remember the original requirements" }] } });
  const source: LiveSession = { handle: { id: "source", attachmentGeneration: "1" }, workspaceId: "workspace", snapshot: snapshot("source", "codex"), projection, inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
  useStore.setState({ ...initial, sessions: { source }, sessionOrder: ["source"], view: { kind: "session", sessionId: "source" }, drafts: { source: "Carry on" }, attachments: {}, mentions: {}, teams: {}, jobs: {}, odyssey: {}, pendingProvider: { source: { provider: "claude" } }, sideBrief: {}, continuations: {}, continuationHistory: {}, error: null,
    loadTeam: vi.fn(async () => undefined), loadOdyssey: vi.fn(async () => undefined), loadNesting: vi.fn(async () => undefined), renameSession: vi.fn(async () => undefined), archiveSession: vi.fn(async () => undefined),
  } as never);
  vi.spyOn(api, "sessionSnapshot").mockImplementation(async (handle) => useStore.getState().sessions[handle.id]?.snapshot ?? snapshot(handle.id, "claude"));
  vi.spyOn(api, "delegationJobs").mockResolvedValue([]);
  vi.spyOn(api, "odysseyForSession").mockResolvedValue(null);
  vi.spyOn(api, "sessionOpen").mockResolvedValue({ handle: { id: "next", attachmentGeneration: "1" }, snapshot: snapshot("next", "claude"), desktopSessionId: "row-next" });
  vi.spyOn(api, "sessionSubscribe").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionHistory").mockResolvedValue([]);
  vi.spyOn(api, "continuitySave").mockRejectedValue({ code: "UNSUPPORTED", message: "not available from the phone", retry: "never" });
  vi.spyOn(api, "continuityAttach").mockRejectedValue({ code: "UNSUPPORTED", message: "not available from the phone", retry: "never" });
  vi.spyOn(api, "prefSet").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionStop").mockResolvedValue({ status: 0, signal: null, forced: false });
  vi.spyOn(api, "sessionSubmit").mockResolvedValue({ outcome: { outcome: "accepted" } } as never);
});

it("changes provider from the phone with the excerpt and asks for no archive", async () => {
  await useStore.getState().continueOnProvider("source");
  expect(api.continuitySave).not.toHaveBeenCalled();
  expect(api.continuityAttach).not.toHaveBeenCalled();
  const prompt = vi.mocked(api.sessionSubmit).mock.calls[0]![2];
  expect(prompt).toContain("Remember the original requirements");
  expect(prompt).toContain("Carry on");
  expect(prompt).not.toContain("read_continuity");
  expect(useStore.getState().error).toBeNull();
  expect(useStore.getState().continuations["agent-next"]).toMatchObject({ fromAgentSessionId: "agent-source", fromProvider: "codex" });
});
