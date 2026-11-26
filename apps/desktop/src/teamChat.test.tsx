/**
 * The agents' chat as the person sees it: who said what to whom, read-only,
 * and a plain line when the host has stopped it.
 */
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { TeamChatMessage, TeamChatView } from "@thingmaker/contracts";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async () => []),
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => undefined) }));

import { TeamChat, TeamChatBand } from "./components/TeamChat";
import { api } from "./ipc";
import { useStore, type LiveSession } from "./store";

const handle = { id: "lead", attachmentGeneration: "1" };
const question: TeamChatMessage = {
  id: 1, from: "job-1", fromName: "Builder", fromProvider: "codex", to: "job-2", toName: "Reviewer",
  text: "Which field?", replyTo: null, sentAt: 1, delivered: true, error: null,
};
const view = (extra: Partial<TeamChatView> = {}): TeamChatView => ({ messages: [], unpromptedLimit: 6, messageLimit: 60, ...extra });

beforeEach(() => {
  useStore.setState({ sessions: { lead: { handle } as LiveSession }, teamChat: {} });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

it("shows who asked and replied, without a composer or a delivery action", async () => {
  const chat = vi.spyOn(api, "delegationChat").mockResolvedValue(view({ messages: [question, {
    ...question, id: 2, from: question.to, fromName: question.toName, to: question.from,
    toName: question.fromName, replyTo: 1, text: "customerId",
  }] }));
  render(<TeamChat sessionId="lead" />);
  expect(await screen.findByText("customerId")).toBeDefined();
  expect(screen.getByText("Builder → Reviewer")).toBeDefined();
  expect(screen.getByText(/↳ #1/)).toBeDefined();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button")).toBeNull();
  expect(chat).toHaveBeenCalledWith(handle);
});

it("says why a message was not delivered", async () => {
  vi.spyOn(api, "delegationChat").mockResolvedValue(view({ messages: [{ ...question, delivered: false, error: "Reviewer did not take it within 15 minutes." }] }));
  render(<TeamChat sessionId="lead" />);
  expect(await screen.findByText("Not delivered: Reviewer did not take it within 15 minutes.")).toBeDefined();
});

it("tells the person when the agents have been stopped, in the list and above the conversation", async () => {
  vi.spyOn(api, "delegationChat").mockResolvedValue(view({ messages: [question], stopped: "paused" }));
  render(<><TeamChatBand sessionId="lead" /><TeamChat sessionId="lead" /></>);
  expect(await screen.findAllByText(/Agent chat is paused: 6 messages in a row started work without you/)).toHaveLength(2);
  cleanup();
  useStore.setState({ teamChat: { lead: view({ stopped: "full" }) } });
  render(<TeamChatBand sessionId="lead" />);
  expect(screen.getByText(/limit of 60 messages/)).toBeDefined();
  cleanup();
  useStore.setState({ teamChat: { lead: view() } });
  const { container } = render(<TeamChatBand sessionId="lead" />);
  expect(container.textContent).toBe("");
});

it("leaves the chat as it was when the host cannot say", async () => {
  vi.spyOn(api, "delegationChat").mockRejectedValue(new Error("no team socket"));
  render(<TeamChat sessionId="lead" />);
  expect(await screen.findByText("Could not refresh team chat.")).toBeDefined();
});
