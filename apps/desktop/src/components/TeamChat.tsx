/**
 * What the agents of a session said to each other: a read-only list in the
 * Agents tab, and a line above the conversation when they can say no more.
 * The host keeps the chat and bounds it; this only shows it.
 */
import { useEffect, useState, type ReactNode } from "react";
import { PROVIDER_LABELS, type TeamChatMessage } from "@thingmaker/contracts";
import { useStore } from "../store";
import { t } from "../i18n";
import { ProviderMark } from "./ProviderMark";

const NONE: TeamChatMessage[] = [];

/** Why the agents can send no more, in the person's words. */
function stopLine(stopped: "paused" | "full", unprompted: number, total: number): string {
  return stopped === "paused"
    ? t("Agent chat is paused: {count} messages in a row started work without you. It resumes when you write in this session.", { count: unprompted })
    : t("Agent chat has reached this session's limit of {count} messages. The agents only report to the lead from here on.", { count: total });
}

/** An agent's name as the person reads it: the lead has an address, not a name of its own. */
function agentName(address: string, name: string): string {
  return address === "orchestrator" ? t("Orchestrator") : name;
}

/**
 * A prompt in the transcript that is shaped like another agent's message.
 *
 * With the host's receipt for its token it is drawn where an answer is, on
 * the left under the sender's provider mark and name: it is not something the
 * person typed, and must not look it. Without one it is the person's own
 * text, and is drawn as theirs (`own`). Nothing is drawn while the host is
 * being asked, which is one read when the card first appears.
 */
export function TeamChatTurn({ sessionId, token, own, children }: { sessionId: string; token: string; own: ReactNode; children: ReactNode }) {
  const receipt = useStore((s) => s.teamChat[sessionId]?.received?.find((entry) => entry.token === token));
  const unknown = useStore((s) => s.teamChatUnknown[sessionId]?.includes(token) ?? false);
  const checkTeamChat = useStore((s) => s.checkTeamChat);
  useEffect(() => {
    if (!receipt && !unknown) void checkTeamChat(sessionId, token);
  }, [receipt, unknown, checkTeamChat, sessionId, token]);
  if (!receipt) return unknown ? <>{own}</> : null;
  const name = agentName(receipt.from, receipt.fromName);
  return (
    <article className="card card-agent card-agent-marked card-team-chat">
      <span className="card-author" title={`${name} · ${PROVIDER_LABELS[receipt.fromProvider]}`}>
        <ProviderMark provider={receipt.fromProvider} size={15} />
      </span>
      <div className="card-content">
        <header className="small team-chat-from" title={t("Sent by another agent of this team, not by you.")}>
          <strong>{name}</strong> <span className="muted">· {t("agent chat")}</span>
        </header>
        {children}
      </div>
    </article>
  );
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
      <div><strong>{agentName(message.from, message.fromName)} → {agentName(message.to, message.toName)}</strong>{" "}
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
