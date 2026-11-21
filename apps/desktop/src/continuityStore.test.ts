import { beforeEach, describe, expect, it, vi } from "vitest";
import type { OpenResponse, Snapshot } from "@thingmaker/contracts";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage: unknown = null; } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
import { api, type OpenRequest } from "./ipc";
import { useStore, sessionContinuation, EMPTY_SESSION_USAGE, type LiveSession } from "./store";
import { emptyProjection, EMPTY_QUEUE } from "./projection";

const initial = useStore.getState();
const sourceHandle = { id: "source", attachmentGeneration: "1" };
const targetHandle = { id: "next", attachmentGeneration: "1" };
const archive = { id: "00000000-0000-4000-8000-000000000001", path: "C:/private/context.md", bytes: 100, createdAtUnixMs: 1 };
let destination: Snapshot;
function snapshot(id = "source", provider: Snapshot["provider"] = "codex"): Snapshot {
  return { handle: { id, attachmentGeneration: "1" }, provider, accountId: "default", agentSessionId: `agent-${id}`, process: "ready", attachment: "attached", foreground: "idle", autonomousTurns: [], detachedCalls: {}, toolCalls: {}, backgroundTasks: [], configOptions: [], capabilities: null, availableCommands: [], pendingSteers: [], queue: EMPTY_QUEUE, lastSequence: "0", historyStart: "0", historyDropped: 0, exit: null };
}
function opened(request: OpenRequest): OpenResponse {
  destination = snapshot("next", request.provider);
  return { handle: targetHandle, snapshot: destination, desktopSessionId: "row-next" };
}
beforeEach(() => {
  vi.restoreAllMocks();
  const projection = emptyProjection();
  Object.assign(projection, { process: "ready", attachment: "attached", foreground: "idle" });
  projection.cards.push({ kind: "message", key: "u", message: { key: "u", messageId: "u", role: "user", contentUnknown: false, blocks: [{ type: "text", text: "Remember the original requirements" }] } });
  const source: LiveSession = { handle: sourceHandle, workspaceId: "workspace", snapshot: snapshot(), projection, inFlightRequestId: null, steerInFlight: false, attention: "none", openedAt: 0, turnStartedAt: null, lastEventAt: null, usage: EMPTY_SESSION_USAGE };
  useStore.setState({ ...initial, sessions: { source }, sessionOrder: ["source"], view: { kind: "session", sessionId: "source" }, drafts: { source: "NEW_UNSENT_REQUEST" }, attachments: {}, mentions: {}, teams: {}, jobs: {}, odyssey: {}, pendingProvider: { source: { provider: "gemini" } }, sideBrief: {}, continuations: {}, continuationHistory: {}, error: null,
    loadTeam: vi.fn(async () => undefined), loadOdyssey: vi.fn(async () => undefined), loadNesting: vi.fn(async () => undefined), renameSession: vi.fn(async () => undefined), archiveSession: vi.fn(async () => undefined),
  } as never);
  vi.spyOn(api, "sessionSnapshot").mockImplementation(async (handle) => handle.id === "source" ? useStore.getState().sessions.source!.snapshot : destination);
  vi.spyOn(api, "delegationJobs").mockResolvedValue([]);
  vi.spyOn(api, "odysseyForSession").mockResolvedValue(null);
  vi.spyOn(api, "sessionOpen").mockImplementation(async (request) => opened(request));
  vi.spyOn(api, "sessionSubscribe").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionHistory").mockResolvedValue([]);
  vi.spyOn(api, "continuitySave").mockResolvedValue(archive);
  vi.spyOn(api, "continuityAttach").mockResolvedValue(undefined);
  vi.spyOn(api, "prefSet").mockResolvedValue(undefined);
  vi.spyOn(api, "sessionStop").mockResolvedValue({ status: 0, signal: null, forced: false });
  vi.spyOn(api, "sessionSubmit").mockResolvedValue({ outcome: { outcome: "accepted" } } as never);
});

describe("leader continuity integration", () => {
  it("archives native jobs and the latest goal even when renderer caches are stale", async () => {
    useStore.setState({ jobs: { source: [] }, odyssey: { source: { goal: { state: "complete", title: "STALE_GOAL" } } } } as never);
    vi.mocked(api.delegationJobs).mockResolvedValue([{ status: "succeeded", result: "NATIVE_RESULT_TOKEN" }] as never);
    vi.mocked(api.odysseyForSession).mockResolvedValue({ goal: { state: "complete", title: "LATEST_GOAL_TOKEN" } } as never);
    await useStore.getState().continueOnProvider("source");
    const markdown = vi.mocked(api.continuitySave).mock.calls[0]![1];
    expect(markdown).toContain("NATIVE_RESULT_TOKEN");
    expect(markdown).toContain("succeeded");
    expect(markdown).toContain("LATEST_GOAL_TOKEN");
    expect(markdown).not.toContain("STALE_GOAL");
    expect(api.odysseyForSession).toHaveBeenCalledWith("workspace", "agent-source");
  });

  it("saves visible history before opening, binds before sending and preserves the archive reference", async () => {
    await useStore.getState().continueOnProvider("source");
    expect(api.continuitySave).toHaveBeenCalledWith(sourceHandle, expect.stringContaining("Remember the original requirements"));
    expect(vi.mocked(api.continuitySave).mock.calls[0]![1]).not.toContain("NEW_UNSENT_REQUEST");
    expect(api.continuityAttach).toHaveBeenCalledWith(sourceHandle, targetHandle, archive.id);
    expect(vi.mocked(api.continuitySave).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.sessionOpen).mock.invocationCallOrder[0]!);
    expect(vi.mocked(api.continuityAttach).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.sessionSubmit).mock.invocationCallOrder[0]!);
    expect(api.sessionSubmit).toHaveBeenCalledWith(targetHandle, expect.any(String), expect.stringContaining(archive.id), [], []);
    expect(vi.mocked(api.sessionSubmit).mock.calls[0]![2]).toContain("NEW_UNSENT_REQUEST");
    expect(useStore.getState().continuations["agent-next"]?.archive).toEqual(archive);
  });

  it("keeps the original conversation and draft when archive creation fails", async () => {
    vi.mocked(api.continuitySave).mockRejectedValueOnce(new Error("Disk full"));
    await useStore.getState().continueOnProvider("source");
    expect(api.sessionOpen).not.toHaveBeenCalled();
    expect(api.sessionSubmit).not.toHaveBeenCalled();
    expect(useStore.getState().drafts.source).toBe("NEW_UNSENT_REQUEST");
    expect(useStore.getState().sessions.source).toBeTruthy();
  });

  it("discards only the unused target when attaching fails", async () => {
    vi.mocked(api.continuityAttach).mockRejectedValueOnce(new Error("Cannot bind"));
    await useStore.getState().continueOnProvider("source");
    expect(api.sessionStop).toHaveBeenCalledExactlyOnceWith(targetHandle);
    expect(api.sessionSubmit).not.toHaveBeenCalled();
    expect(useStore.getState().sessions.source).toBeTruthy();
    expect(useStore.getState().sessions.next).toBeUndefined();
    expect(useStore.getState().drafts.source).toBe("NEW_UNSENT_REQUEST");
  });

  it("preserves both sessions after an ambiguous submit error", async () => {
    vi.mocked(api.sessionSubmit).mockRejectedValueOnce(new Error("Connection lost"));
    await useStore.getState().continueOnProvider("source");
    expect(api.sessionStop).not.toHaveBeenCalled();
    expect(useStore.getState().sessions.source).toBeTruthy();
    expect(useStore.getState().sessions.next).toBeTruthy();
    expect(useStore.getState().drafts.source).toBe("NEW_UNSENT_REQUEST");
  });

  it("keeps composer edits made while the target opens", async () => {
    vi.mocked(api.sessionOpen).mockImplementationOnce(async (request) => {
      useStore.setState({ drafts: { source: "A newer unsent edit" } });
      return opened(request);
    });
    await useStore.getState().continueOnProvider("source");
    expect(useStore.getState().drafts.source).toBe("A newer unsent edit");
    expect(useStore.getState().sessions.source).toBeTruthy();
    expect(api.sessionStop).not.toHaveBeenCalled();
  });

  it("allows the limited excerpt when automatic continuity is explicitly off", async () => {
    useStore.setState({ uiPrefs: { ...initial.uiPrefs, autoContinuity: false } });
    await useStore.getState().continueOnProvider("source");
    expect(api.continuitySave).not.toHaveBeenCalled();
    expect(api.continuityAttach).not.toHaveBeenCalled();
    expect(api.sessionSubmit).toHaveBeenCalled();
    expect(vi.mocked(api.sessionSubmit).mock.calls[0]![2]).not.toContain("read_continuity");
  });

  it("does not pretend a local chat can read an archive", async () => {
    vi.mocked(api.sessionOpen).mockImplementationOnce(async (request) => {
      const response = opened(request);
      destination.provider = "local";
      destination.capabilities = { agentName: "thingmaker-local-chat" } as never;
      return response;
    });
    await useStore.getState().continueOnProvider("source");
    expect(api.sessionSubmit).not.toHaveBeenCalled();
    expect(api.continuityAttach).not.toHaveBeenCalled();
    expect(useStore.getState().error?.code).toBe("UNSUPPORTED");
    expect(useStore.getState().sessions.source).toBeTruthy();
  });

  it("prepares explicit handoff with a persistent hidden brief, without submitting it", async () => {
    await useStore.getState().handOff("source", "gemini");
    expect(api.sessionSubmit).not.toHaveBeenCalled();
    expect(api.continuityAttach).toHaveBeenCalled();
    expect(useStore.getState().drafts.next).not.toContain("read_continuity");
    expect(useStore.getState().sideBrief.next).toContain(archive.id);
    const saved = useStore.getState().continuations;
    vi.spyOn(api, "prefGet").mockResolvedValueOnce(saved);
    useStore.setState({ continuations: {}, sideBrief: {} });
    await useStore.getState().loadContinuations();
    expect(useStore.getState().sideBrief.next).toContain(archive.id);
  });

  it("preserves inherited context when switching an empty prepared continuation again", async () => {
    useStore.getState().sessions.source!.projection.cards = [];
    useStore.setState({ sideBrief: { source: "The previous leader's pending briefing" } });
    await useStore.getState().continueOnProvider("source");
    expect(api.continuitySave).toHaveBeenCalled();
    expect(api.continuityAttach).toHaveBeenCalled();
    expect(vi.mocked(api.sessionSubmit).mock.calls[0]![2]).toContain("previous leader's pending briefing");
  });

  it("exports the entire inherited document using the reader's Unicode offsets", async () => {
    vi.spyOn(api, "continuityRead")
      .mockResolvedValueOnce({ archive, text: "first😀", nextOffset: 6, totalChars: 10 })
      .mockResolvedValueOnce({ archive, text: "last", nextOffset: null, totalChars: 10 });
    vi.spyOn(api, "saveTextFile").mockResolvedValueOnce("C:/chosen/continuity.md");
    await useStore.getState().exportContinuity("source");
    expect(api.continuityRead).toHaveBeenNthCalledWith(2, sourceHandle, archive.id, 6, 24000);
    expect(api.saveTextFile).toHaveBeenCalledWith(`continuity-${archive.id}.md`, "first😀last");
    expect(api.sessionSubmit).not.toHaveBeenCalled();
  });

  it("marks native history loss while retaining available cached early messages", async () => {
    useStore.getState().sessions.source!.snapshot.historyDropped = 3000;
    await useStore.getState().continueOnProvider("source");
    const text = vi.mocked(api.continuitySave).mock.calls[0]![1];
    expect(text).toContain("dropped 3000 earlier events");
    expect(text).toContain("Remember the original requirements");
  });

  it("rejects an archive made stale by a completed source turn during startup", async () => {
    vi.mocked(api.sessionOpen).mockImplementationOnce(async (request) => {
      useStore.getState().sessions.source!.snapshot.lastSequence = "1";
      return opened(request);
    });
    await useStore.getState().continueOnProvider("source");
    expect(api.sessionSubmit).not.toHaveBeenCalled();
    expect(api.sessionStop).toHaveBeenCalledExactlyOnceWith(targetHandle);
    expect(useStore.getState().sessions.source).toBeTruthy();
    expect(useStore.getState().drafts.source).toBe("NEW_UNSENT_REQUEST");
  });

  it("does not restore legacy private briefs or another account's scoped continuation", async () => {
    const source = useStore.getState().sessions.source!;
    const legacy = { fromAgentSessionId: "old", fromProvider: "claude" as const, pendingBrief: "PRIVATE_LEGACY_TEXT" };
    vi.spyOn(api, "prefGet").mockResolvedValueOnce({ "agent-source": legacy });
    await useStore.getState().loadContinuations();
    expect(useStore.getState().sideBrief.source).toBeUndefined();
    expect(sessionContinuation(source, { "agent-source": legacy })).toBeUndefined();
    expect(sessionContinuation(source, { "agent-source": { ...legacy, archive, target: { workspaceId: "workspace", provider: "codex", accountId: "another-account" } } })).toBeUndefined();
  });

  it("serializes durable metadata writes for independent simultaneous handoffs", async () => {
    const second = structuredClone(useStore.getState().sessions.source!);
    second.handle = { id: "source2", attachmentGeneration: "1" };
    second.snapshot = snapshot("source2");
    useStore.setState({ sessions: { ...useStore.getState().sessions, source2: second } });
    const destinations: Record<string, Snapshot> = {};
    let count = 0;
    vi.mocked(api.sessionOpen).mockImplementation(async (request) => {
      const next = snapshot(`next${++count}`, request.provider);
      destinations[next.handle.id] = next;
      return { handle: next.handle, snapshot: next, desktopSessionId: next.handle.id };
    });
    vi.mocked(api.sessionSnapshot).mockImplementation(async (handle) => {
      const source = useStore.getState().sessions[handle.id];
      return destinations[handle.id] ?? source!.snapshot;
    });
    let release!: () => void;
    vi.mocked(api.prefSet).mockImplementationOnce(() => new Promise<void>((resolve) => { release = resolve; }));
    const first = useStore.getState().handOff("source", "gemini");
    const other = useStore.getState().handOff("source2", "claude");
    await vi.waitFor(() => expect(api.prefSet).toHaveBeenCalledTimes(1));
    release();
    await Promise.all([first, other]);
    const saved = vi.mocked(api.prefSet).mock.calls.at(-1)![1] as Record<string, unknown>;
    expect(saved).toHaveProperty("agent-next1");
    expect(saved).toHaveProperty("agent-next2");
    expect(useStore.getState().continuations).toHaveProperty("agent-next1");
    expect(useStore.getState().continuations).toHaveProperty("agent-next2");
  });
});
