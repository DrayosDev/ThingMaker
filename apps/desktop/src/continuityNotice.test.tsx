import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(), Channel: class { onmessage: unknown = null; } }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));
import { useStore } from "./store";
import { emptyProjection } from "./projection";
import { ContinuityNotice } from "./components/ContinuityNotice";
import { QuotaRecovery } from "./components/QuotaRecovery";

const exportContinuity = vi.fn();
const target = { workspaceId: "w", provider: "codex", accountId: "default" };
beforeEach(() => {
  exportContinuity.mockReset();
  const projection = emptyProjection();
  projection.quotaFailure = true;
  useStore.setState({ sessions: { s: { workspaceId: "w", snapshot: { agentSessionId: "agent", provider: "codex" }, projection } }, continuations: {}, exportContinuity, uiPrefs: {}, accounts: {}, providers: [], loadAccounts: vi.fn(async () => {}) } as never);
});
afterEach(cleanup);
it("only shows an archive bound to this session", () => {
  useStore.setState({ continuations: { other: { archive: { path: "elsewhere" } } } } as never);
  const { rerender } = render(<ContinuityNotice sessionId="s" />);
  expect(screen.queryByRole("region", { name: "Continuity archive" })).toBeNull();
  useStore.setState({ continuations: { agent: { target, archive: { path: "private/path.md" } } } } as never);
  rerender(<ContinuityNotice sessionId="s" />);
  fireEvent.click(screen.getByText("Archive location"));
  expect(screen.getByText("private/path.md")).toBeTruthy();
});
it("exports by keyboard and disables repeat actions while busy", async () => {
  let finish!: () => void;
  exportContinuity.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
  useStore.setState({ continuations: { agent: { target, archive: { path: "private/path.md" } } } } as never);
  render(<ContinuityNotice sessionId="s" />);
  const button = screen.getByRole("button", { name: "Export continuity…" });
  button.focus();
  expect(document.activeElement).toBe(button);
  fireEvent.click(button);
  expect(exportContinuity).toHaveBeenCalledOnce();
  expect(exportContinuity).toHaveBeenCalledWith("s");
  expect((screen.getByRole("button", { name: "Exporting…" }) as HTMLButtonElement).disabled).toBe(true);
  finish();
  await waitFor(() => expect(screen.getByRole("button", { name: "Export continuity…" })).toHaveProperty("disabled", false));
});
it("describes archive or limited summary according to the preference", () => {
  const { rerender } = render(<QuotaRecovery sessionId="s" />);
  fireEvent.click(screen.getByRole("button", { name: "Continue with another account or provider" }));
  expect(screen.getByText(/recent excerpt and can read the private archive/)).toBeTruthy();
  useStore.setState({ uiPrefs: { autoContinuity: false } } as never);
  rerender(<QuotaRecovery sessionId="s" />);
  expect(screen.getByText(/receives a limited summary/)).toBeTruthy();
});
