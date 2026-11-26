/**
 * Which prompts in a transcript another agent of the team sent.
 *
 * A message between agents reaches its recipient as a prompt, like anything
 * an agent is sent, so the transcript holds it as a user message: live under
 * the id the host submitted it with, and under the provider's own id once the
 * session is reopened from its history. The host wraps the text
 * (`delegation/jobs/chat.rs`, `wrap`) and this reads the wrapper back.
 *
 * Reading it is not enough to show it as an agent's: anyone can type the
 * wrapper. The first line carries a token, and the host keeps a receipt for
 * every token it wrote into a conversation. A prompt is another agent's only
 * when its token has a receipt in this session (`TeamChatTurn`).
 */

export type TeamChatTurn = {
  /** What the host's receipt is looked up by. */
  token: string;
  /** What the sender wrote. */
  body: string;
};

const OPENING = /^\s*<team-chat id="([0-9a-f]{32})"[^\n]*>\n/;
const CLOSING = "\n</team-chat>";

/**
 * The agent message a prompt's text holds, if it is shaped like one. What the
 * sender wrote runs to the last closing line, so a closing line of its own
 * cannot end the message early; the host's note to the model after it (whose
 * words these are, how to reply) is left out.
 */
export function teamChatOf(text: string): TeamChatTurn | null {
  const opening = OPENING.exec(text);
  if (!opening) return null;
  const rest = text.slice(opening[0].length);
  const end = `\n${rest}`.lastIndexOf(CLOSING);
  if (end < 0) return null;
  return { token: opening[1]!, body: rest.slice(0, end > 0 ? end - 1 : 0) };
}
