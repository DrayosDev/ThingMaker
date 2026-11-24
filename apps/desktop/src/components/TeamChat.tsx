import { useEffect, useState } from "react";
import type { SessionHandle, TeamChatMessage } from "@thingmaker/contracts";
import { api } from "../ipc";
import { t } from "../i18n";

export function TeamChat({ handle }: { handle: SessionHandle }) {
  const [messages, setMessages] = useState<TeamChatMessage[]>([]);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    async function refresh() {
      try {
        const result = await api.delegationChat(handle);
        if (active) {
          setMessages(result);
          setFailed(false);
        }
      } catch {
        if (active) setFailed(true);
      } finally {
        if (active) timer = setTimeout(refresh, 1500);
      }
    }
    void refresh();
    return () => { active = false; clearTimeout(timer); };
  }, [handle]);
  return <section>
    <h3>{t("Team chat")}</h3>
    {failed && <p role="status">{t("Could not refresh team chat.")}</p>}
    {!failed && !messages.length && <p className="small muted">{t("No messages yet.")}</p>}
    <ol className="jobs-list">{messages.map(message => <li key={message.id} className="job-row">
      <div><strong>{message.fromName} → {message.toName}</strong>{" "}
        <small className="muted">#{message.id}{message.replyTo && ` ↳ #${message.replyTo}`} · {new Date(message.sentAt).toLocaleTimeString()}</small>
      </div>
      <p style={{whiteSpace:"pre-wrap", overflowWrap:"anywhere"}}>{message.text}</p>
      <small className="muted">{message.delivered ? t("Delivered") : [t("Pending"), message.error].filter(Boolean).join(" · ")}</small>
    </li>)}</ol>
  </section>;
}
