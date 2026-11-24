import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";
import { TeamChat } from "./components/TeamChat";
import { api } from "./ipc";

vi.mock("./ipc", () => ({ api: { delegationChat: vi.fn() } }));
afterEach(cleanup);

it("shows who asked and replied, without a composer or a delivery action", async () => {
  const handle = { id: "lead", attachmentGeneration: "1" };
  const question = {
    id: 1, from: "job-1", fromName: "Builder", to: "job-2", toName: "Reviewer",
    text: "Which field?", replyTo: null, sentAt: 1, delivered: true, error: null,
  };
  vi.mocked(api.delegationChat).mockResolvedValue([question, {
    ...question, id: 2, from: question.to, fromName: question.toName, to: question.from,
    toName: question.fromName, replyTo: 1, text: "customerId",
  }]);
  render(<TeamChat handle={handle} />);
  expect(await screen.findByText("customerId")).toBeDefined();
  expect(screen.getByText("Builder → Reviewer")).toBeDefined();
  expect(screen.getByText(/↳ #1/)).toBeDefined();
  expect(screen.queryByRole("textbox")).toBeNull();
  expect(screen.queryByRole("button")).toBeNull();
  expect(api.delegationChat).toHaveBeenCalledWith(handle);
});
