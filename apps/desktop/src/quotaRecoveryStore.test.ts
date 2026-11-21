import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenResponse, Snapshot } from "@thingmaker/contracts";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage: unknown = null; } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
import { api, type OpenRequest } from "./ipc";
import { useStore, EMPTY_SESSION_USAGE, type LiveSession } from "./store";
import { emptyProjection, EMPTY_QUEUE } from "./projection";

const initial = useStore.getState();
const team = { workers: [{ name: "Reviewer", provider: "claude" as const, capabilities: ["review"] }], nativeSubagents: false };
const sourceHandle = { id: "source", attachmentGeneration: "1" };
const targetHandle = { id: "next", attachmentGeneration: "1" };
let destination: Snapshot;
function snapshot(id = "source", provider: Snapshot["provider"] = "codex", accountId = "default"): Snapshot {
  return { handle: { id, attachmentGeneration: "1" }, provider, accountId, agentSessionId: `agent-${id}`, process: "ready", attachment: "attached", foreground: "failed", autonomousTurns: [], detachedCalls: {}, toolCalls: {}, backgroundTasks: [], configOptions: [{ id: "model", currentValue: "cx-model" }, { id: "effort", currentValue: "high" }], capabilities: null, availableCommands: [], pendingSteers: [], queue: EMPTY_QUEUE, lastSequence: "0", historyStart: "0", historyDropped: 0, exit: null };
}
function opened(request: OpenRequest): OpenResponse {
  destination = snapshot("next", request.provider, request.accountId);
  destination.foreground = "idle";
  return { handle: targetHandle, snapshot: destination, desktopSessionId: "row-next" };
}

beforeEach(() => {
  vi.restoreAllMocks();
  const projection = emptyProjection();
  Object.assign(projection, { process: "ready", attachment: "attached", foreground: "failed", quotaFailure: true });
  projection.cards.push({ kind: "message", key: "user-1", message: { key: "user-1", messageId: "1", role: "user", contentUnknown: false, blocks: [{ type: "text", text: "Finish the pending feature" }] } });
  const source: LiveSession = { handle: sourceHandle, workspaceId: "workspace", snapshot: snapshot(), projection, inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
  useStore.setState({ ...initial, sessions: { source }, sessionOrder: ["source"], view: { kind: "session", sessionId: "source" }, drafts: { source: "Do not send this draft yet" }, attachments: { source: [{ id: "image-id", name: "image.png", kind: "image", mime: "image/png", bytes: 10 }] }, mentions: { source: [{ relativePath: "src/main.ts", mode: { mode: "path_reference" } }] }, teams: { source: team }, jobs: {}, odyssey: {}, pendingProvider: {}, sideBrief: {}, continuations: {}, continuationHistory: {}, error: null, providers: ["codex", "gemini"].map((provider) => ({ provider, resolved: { program: provider }, auth: { loggedIn: true } })), loadTeam: vi.fn(async () => undefined), loadOdyssey: vi.fn(async () => undefined), loadNesting: vi.fn(async () => undefined), renameSession: vi.fn(async () => undefined) } as never);
  vi.spyOn(api, "accountsList").mockImplementation(async (provider) => [
    { id: "default", provider, isDefault: true, active: true, label: "Main", auth: { provider, loggedIn: true } },
    { id: "work", provider, isDefault: false, active: false, label: "Work", auth: { provider, loggedIn: true } },
  ]);
  vi.spyOn(api, "sessionSnapshot").mockImplementation(async (handle) => handle.id === "source" ? useStore.getState().sessions.source!.snapshot : destination);
  vi.spyOn(api, "delegationJobs").mockResolvedValue([]);
  vi.spyOn(api, "delegationComboGet").mockResolvedValue(team);
  vi.spyOn(api, "odysseyForSession").mockResolvedValue(null);
  vi.spyOn(api, "sessionOpen").mockImplementation(async (request) => opened(request));
  vi.spyOn(api, "sessionSubscribe").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionHistory").mockResolvedValue([]);
  vi.spyOn(api, "prefSet").mockResolvedValue(undefined);
  vi.spyOn(api, "accountSetActive").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionStop").mockResolvedValue({ status: 0, signal: null, forced: false });
  vi.spyOn(api, "sessionSubmit").mockResolvedValue({ outcome: { outcome: "accepted" } } as never);
});

describe("preparing a continuation after quota exhaustion", () => {
  it("preserves the source, team and composer and sends nothing when changing provider", async () => {
    const before = useStore.getState().sessions.source;
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBe("next");
    expect(api.sessionOpen).toHaveBeenCalledWith(expect.objectContaining({ provider: "gemini", accountId: "default", mode: { mode: "new" }, combo: team }));
    const state = useStore.getState();
    expect(state.sessions.source).toBe(before);
    expect(state.drafts.next).toBe(state.drafts.source);
    expect(state.attachments.next).toEqual(state.attachments.source);
    expect(state.mentions.next).toEqual(state.mentions.source);
    expect(state.teams.next).toEqual(team);
    expect(state.sideBrief.next).toContain("Finish the pending feature");
    expect(state.continuations["agent-next"]?.pendingBrief).toBe(state.sideBrief.next);
    expect(api.sessionSubmit).not.toHaveBeenCalled();
    expect(api.accountSetActive).not.toHaveBeenCalled();
    expect(api.sessionStop).not.toHaveBeenCalled();
  });

  it("pins another account in a fresh session without changing the global account", async () => {
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "codex", accountId: "work" })).toBe("next");
    expect(api.sessionOpen).toHaveBeenCalledWith(expect.objectContaining({ accountId: "work", mode: { mode: "new" }, model: "cx-model", reasoningEffort: "high" }));
    expect(useStore.getState().sessions.next?.snapshot.accountId).toBe("work");
    expect(api.accountSetActive).not.toHaveBeenCalled();
    expect(api.sessionStop).not.toHaveBeenCalled();
  });

  it("retains all source state if opening fails", async () => {
    vi.mocked(api.sessionOpen).mockRejectedValueOnce(new Error("account unavailable"));
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBeNull();
    expect(useStore.getState().drafts.source).toBe("Do not send this draft yet");
    expect(useStore.getState().sessions.source).toBeTruthy();
    expect(useStore.getState().attachments.source).toHaveLength(1);
    expect(api.sessionStop).not.toHaveBeenCalled();
  });

  it("rejects a host that ignores the chosen account", async () => {
    vi.mocked(api.sessionOpen).mockImplementationOnce(async (request) => opened({ ...request, accountId: "default" }));
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "codex", accountId: "work" })).toBeNull();
    expect(api.sessionStop).toHaveBeenCalledWith(targetHandle);
    expect(useStore.getState().sessions.source).toBeTruthy();
    expect(api.sessionSubmit).not.toHaveBeenCalled();
  });

  it("guards duplicate activations and carries edits made while the destination opens", async () => {
    let finish!: (response: OpenResponse) => void;
    let request!: OpenRequest;
    vi.mocked(api.sessionOpen).mockImplementationOnce((value) => { request = value; return new Promise((resolve) => { finish = resolve; }); });
    const first = useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" });
    await vi.waitFor(() => expect(api.sessionOpen).toHaveBeenCalledTimes(1));
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBeNull();
    useStore.setState({ drafts: { source: "Updated while loading" } });
    finish(opened(request));
    expect(await first).toBe("next");
    expect(useStore.getState().drafts.next).toBe("Updated while loading");
  });

  it("closes only the unused target if the source starts working during startup", async () => {
    vi.mocked(api.sessionOpen).mockImplementationOnce(async (request) => {
      const source = useStore.getState().sessions.source!;
      useStore.setState({ sessions: { source: { ...source, inFlightRequestId: "user-send" } } });
      return opened(request);
    });
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBeNull();
    expect(api.sessionStop).toHaveBeenCalledTimes(1);
    expect(api.sessionStop).toHaveBeenCalledWith(targetHandle);
    expect(useStore.getState().drafts.source).toBe("Do not send this draft yet");
  });

  it("blocks active workers, queued messages, and goals without opening a destination", async () => {
    vi.mocked(api.delegationJobs).mockResolvedValueOnce([{ status: "waiting" }] as never);
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBeNull();
    vi.mocked(api.odysseyForSession).mockResolvedValueOnce({ goal: { state: "paused" } } as never);
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBeNull();
    useStore.getState().sessions.source!.snapshot.queue = { ...EMPTY_QUEUE, items: [{ id: "queued" }] as never };
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBeNull();
    expect(api.sessionOpen).not.toHaveBeenCalled();
  });

  it("does not infer exhaustion from unavailable usage", async () => {
    useStore.getState().sessions.source!.projection.quotaFailure = false;
    expect(await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" })).toBeNull();
    expect(api.sessionOpen).not.toHaveBeenCalled();
  });

  it("restores a pending brief after reload and consumes it only after an accepted send", async () => {
    await useStore.getState().prepareQuotaContinuation("source", { provider: "gemini" });
    const saved = useStore.getState().continuations;
    vi.spyOn(api, "prefGet").mockResolvedValueOnce(saved);
    useStore.setState({ sideBrief: {}, continuations: {} });
    await useStore.getState().loadContinuations();
    expect(useStore.getState().sideBrief.next).toContain("Finish the pending feature");
    await useStore.getState().send("next");
    expect(api.sessionSubmit).toHaveBeenCalledWith(targetHandle, expect.any(String), expect.stringContaining("Finish the pending feature"), ["image-id"], expect.any(Array));
    expect(useStore.getState().continuations["agent-next"]?.pendingBrief).toBeUndefined();
    expect(useStore.getState().sideBrief.next).toBeUndefined();
    expect(useStore.getState().drafts.source).toBe("Do not send this draft yet");
  });
});
