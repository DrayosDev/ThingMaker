/**
 * What the agents of a session said to each other: a read-only list in the
 * Agents tab, and a line above the conversation when they can say no more.
 * The host keeps the chat and bounds it; this only shows it.
 */
import { useEffect, useState } from "react";
import type { TeamChatMessage } from "@thingmaker/contracts";
import { useStore } from "../store";
import { t } from "../i18n";

const NONE: TeamChatMessage[] = [];

/** Why the agents can send no more, in the person's words. */
function stopLine(stopped: "paused" | "full", unprompted: number, total: number): string {
  return stopped === "paused"
    ? t("Agent chat is paused: {count} messages in a row started work without you. It resumes when you write in this session.", { count: unprompted })
    : t("Agent chat has reached this session's limit of {count} messages. The agents only report to the lead from here on.", { count: total });
}

export function TeamChat({ sessionId }: { sessionId: string }) {
  const view = useStore((s) => s.teamChat[sessionId]);
  const loadTeamChat = useStore((s) => s.loadTeamChat);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      const read = await loadTeamChat(sessionId);
      if (!active) return;
      setFailed(!read);
      timer = setTimeout(refresh, 1500);
    }
    void refresh();
    return () => { active = false; clearTimeout(timer); };
  }, [sessionId, loadTeamChat]);
  const messages = view?.messages ?? NONE;
  return <section>
    <h3>{t("Team chat")}</h3>
    {failed && <p role="status">{t("Could not refresh team chat.")}</p>}
    {view?.stopped && <p className="small chip-warn-text" role="status">{stopLine(view.stopped, view.unpromptedLimit, view.messageLimit)}</p>}
    {!failed && !messages.length && <p className="small muted">{t("No messages yet.")}</p>}
    <ol className="jobs-list">{messages.map(message => <li key={message.id} className="job-row">
      <div><strong>{message.fromName} → {message.toName}</strong>{" "}
        <small className="muted">#{message.id}{message.replyTo && ` ↳ #${message.replyTo}`} · {new Date(message.sentAt).toLocaleTimeString()}</small>
      </div>
      <p style={{whiteSpace:"pre-wrap", overflowWrap:"anywhere"}}>{message.text}</p>
      <small className={message.error ? "chip-warn-text" : "muted"}>{message.delivered ? t("Delivered") : message.error ? t("Not delivered: {reason}", { reason: message.error }) : t("Pending")}</small>
    </li>)}</ol>
  </section>;
}

/**
 * Above the conversation once the agents may send no more: the person is the
 * one who can let them go on, and would otherwise only find out in the
 * Agents tab.
 */
export function TeamChatBand({ sessionId }: { sessionId: string }) {
  const view = useStore((s) => s.teamChat[sessionId]);
  if (!view?.stopped) return null;
  return (
    <div className="bigthing-band state-paused" role="status">
      <span className="bigthing-band-line">
        <strong>{stopLine(view.stopped, view.unpromptedLimit, view.messageLimit)}</strong>
      </span>
    </div>
  );
}
