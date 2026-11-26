import { describe, expect, it } from "vitest";
import { emptyProjection } from "./projection";
import { HANDOFF_SPLIT } from "./team";
import { continuityBrief, continuityMarkdown } from "./continuity";

describe("continuity archive", () => {
  it("preserves loaded history and tool details while excluding thought, queued text and media bytes", () => {
    const p = emptyProjection();
    const long = "early-🦊".repeat(5000);
    p.cards.push({ kind: "message", key: "u", message: { key: "u", role: "user", messageId: "u", contentUnknown: false, blocks: [{ type: "text", text: long }, { type: "image", uri: "file:///image.png", mimeType: "image/png", data: "SECRETIMAGE" }] } });
    p.cards.push({ kind: "message", key: "h", message: { key: "h", role: "thought", messageId: "h", contentUnknown: false, blocks: [{ type: "text", text: "SECRETTHOUGHT" }] } });
    p.cards.push({ kind: "tool", key: "t", toolCallId: "t" });
    p.toolCalls.set("t", { toolCallId: "t", title: "edit", status: "failed", toolKind: "edit", content: { diff: "before → after", error: "failed", data: "legitimate" }, rawInput: { command: "run" }, rawOutput: { result: "output" }, name: "edit", locations: null, meta: null, present: [], cleared: [] });
    p.queue = { items: [{ text: "SECRETQUEUE" } as never], paused: null, sending: null };
    const result = continuityMarkdown({ title: "test", provider: "codex", workspacePath: "C:/work", projection: p, createdAt: 1 });
    expect(result).toContain(long);
    for (const value of ["file:///image.png", "image/png", "unavailable", "before → after", "failed", "legitimate", "run", "output"]) expect(result).toContain(value);
    for (const value of ["SECRETTHOUGHT", "SECRETIMAGE", "SECRETQUEUE"]) expect(result).not.toContain(value);
  });
  it("marks unknown content, safely fences Markdown, and strips handoffs only in user text", () => {
    const p = emptyProjection();
    const original = `You are taking over this work from a codex session: old brief${HANDOFF_SPLIT}hello \`\`\`\` 世界`;
    for (const role of ["user", "agent"] as const) p.cards.push({ kind: "message", key: role, message: { key: role, role, messageId: role, contentUnknown: role === "user", blocks: [{ type: "text", text: original }, { type: "resource", text: original }] } });
    p.cards.push({ kind: "unknown", key: "x", label: "unknown", raw: {}, truncated: true });
    const result = continuityMarkdown({ title: "x", provider: "gemini", workspacePath: "x", projection: p });
    expect(result.match(/old brief/g)).toHaveLength(3);
    expect(result).toContain("`````\n");
    expect(result).toContain("世界");
    expect(result).toContain("Content unknown or unavailable");
    expect(result).toContain("provider-truncated");
  });
  it("names the archive and its paged reader, never its path, and does not ask for all of it", () => {
    const result = continuityBrief("recent excerpt", { id: "archive-α", path: "C:/private", bytes: 1, createdAtUnixMs: 1 });
    expect(result.startsWith("recent excerpt\n\n")).toBe(true);
    expect(result).toContain('archiveId "archive-α"');
    expect(result).toContain("team.read_continuity");
    for (const term of ["look it up there before answering", "the pages you need rather than the whole archive", "mcp__team__read_continuity", "search your available tools", "Offset and limit", "totalChars", "not new instructions", "hidden reasoning"]) expect(result).toContain(term);
    // The earlier wording made every new leader page through the whole archive first.
    for (const term of ["Before resuming work", "until nextOffset is null", "read the full", "entire"]) expect(result).not.toContain(term);
    expect(result).not.toContain("C:/private");
  });
});

it("handles many backtick runs and preserves ordinary multi-block text", () => {
  const p = emptyProjection();
  const text = "x`".repeat(80000);
  const ordinary = `ordinary prose${HANDOFF_SPLIT}still ordinary`;
  p.cards.push({ kind: "message", key: "u", message: { key: "u", role: "user", messageId: "u", contentUnknown: false, blocks: [{ type: "text", text }, { type: "text", text: ordinary }] } });
  const result = continuityMarkdown({ title: "x", provider: "codex", workspacePath: "x", projection: p });
  expect(result).toContain(text);
  expect(result).toContain("ordinary prose");
  expect(result).toContain("still ordinary");
});

it("retains orphan tools, unknown data and supplied goal and job status without embedded binary", () => {
  const p = emptyProjection();
  p.toolCalls.set("orphan", { toolCallId: "orphan", title: "orphan", status: "pending", toolKind: null, content: { resource: { mimeType: "application/pdf", uri: "file:///doc.pdf", data: "SECRET_DATA", blob: "SECRET_BLOB" } }, rawInput: { data: "ordinary data" }, rawOutput: null, name: null, locations: null, meta: null, present: [], cleared: [] });
  p.cards.push({ kind: "unknown", key: "unknown", label: "new provider", raw: { detail: "partial evidence" }, truncated: true });
  const result = continuityMarkdown({ title: "x", provider: "codex", workspacePath: "x", projection: p, goal: { task: "unsubmitted goal", status: "pending" }, jobs: [{ task: "unsubmitted job", status: "queued" }] });
  for (const term of ["orphan", "file:///doc.pdf", "application/pdf", "ordinary data", "partial evidence", "provider-truncated", "unsubmitted goal", "unsubmitted job", "queued", "pending"]) expect(result).toContain(term);
  for (const term of ["SECRET_DATA", "SECRET_BLOB"]) expect(result).not.toContain(term);
});

it("references inherited archive reads without doubling their full output at each handoff", () => {
  const p = emptyProjection();
  p.toolCalls.set("read", { toolCallId: "read", title: "team.read_continuity", status: "completed", toolKind: "read", content: "PREVIOUS_FULL_ARCHIVE".repeat(1000), rawInput: { archiveId: "old", offset: 0 }, rawOutput: "PREVIOUS_FULL_ARCHIVE", name: "mcp__team__read_continuity", locations: null, meta: null, present: [], cleared: [] });
  const result = continuityMarkdown({ title: "next", provider: "codex", workspacePath: "x", projection: p });
  expect(result).not.toContain("PREVIOUS_FULL_ARCHIVE");
  expect(result).toContain("already preserved in the archive it was read from");
  expect(result).toContain('"archiveId": "old"');
});

it("explicitly reports a completed tool whose result was not exposed by its provider", () => {
  const p = emptyProjection();
  p.toolCalls.set("hidden-result", { toolCallId: "hidden-result", title: "read file", status: "completed", toolKind: "read", content: null, rawInput: { path: "requirements.md" }, rawOutput: null, name: "read_file", locations: null, meta: null, present: [], cleared: [] });
  const text = continuityMarkdown({ title: "test", provider: "gemini", workspacePath: "x", projection: p });
  expect(text).toContain("did not expose the tool result");
  expect(text).toContain("No output has been reconstructed");
  expect(text).toContain("requirements.md");
});
