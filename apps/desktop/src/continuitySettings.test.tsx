import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { useStore } from "./store";
import { ContinuitySettings } from "./components/ContinuitySettings";

afterEach(cleanup);
it("defaults to automatic continuity and disables export with no open conversations", () => {
  const setUiPrefs = vi.fn();
  useStore.setState({ sessions: {}, sessionOrder: [], navHistory: [], view: { kind: "settings", tab: "continuity" }, uiPrefs: {}, setUiPrefs } as never);
  render(<ContinuitySettings />);
  const toggle = screen.getByRole("switch", { name: "Automatic continuity" });
  expect(toggle.getAttribute("aria-checked")).toBe("true");
  fireEvent.click(toggle);
  expect(setUiPrefs).toHaveBeenCalledWith({ autoContinuity: false });
  expect((screen.getByRole("button", { name: "Export Markdown…" }) as HTMLButtonElement).disabled).toBe(true);
});
it("selects another open conversation by keyboard and prevents duplicate exports", async () => {
  let finish!: () => void;
  const exportContinuity = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
  const session = (id: string) => ({ handle: { id }, snapshot: { agentSessionId: null }, projection: { title: id, cards: [] } });
  useStore.setState({ sessions: { one: session("one"), two: session("two") }, sessionOrder: ["one", "two"], navHistory: [], view: { kind: "settings" }, exportContinuity } as never);
  render(<ContinuitySettings />);
  fireEvent.click(screen.getByRole("button", { name: "Conversation" }));
  fireEvent.keyDown(screen.getByRole("option", { name: "one" }), { key: "ArrowDown" });
  expect(document.activeElement).toBe(screen.getByRole("option", { name: "two" }));
  fireEvent.click(document.activeElement!);
  fireEvent.click(screen.getByRole("button", { name: "Export Markdown…" }));
  expect(exportContinuity).toHaveBeenCalledWith("two");
  expect((screen.getByRole("button", { name: "Exporting…" }) as HTMLButtonElement).disabled).toBe(true);
  finish();
  await screen.findByRole("button", { name: "Export Markdown…" });
});
