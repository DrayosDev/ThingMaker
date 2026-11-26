//! Session-scoped mailboxes. Addresses identify a conversation, even when two jobs use the same slot.
//!
//! Two agents answering each other need nobody else to keep going, and every
//! answer is a turn on the person's account. So a session's chat is bounded
//! twice: by how many deliveries in a row may start work without the person
//! writing anything ([`MAX_UNPROMPTED_STARTS`]), and by how many messages it
//! may hold in all ([`MAX_SESSION_MESSAGES`]). A sender past either is told
//! so, and the session's view says it for the person.
use super::*;
use crate::supervisor::SteerOutcome;
use serde_json::{Value, json};
use std::collections::HashSet;

const LEAD: &str = "orchestrator";

/// What a member the team's setting leaves out of the chat is told.
const CHAT_CLOSED: &str = "Agent chat is off for this member of the team. Say it in your report to the orchestrator instead.";

/// Deliveries in a row that may start a turn or a job without the person
/// writing anything in the session. A message steered into a turn that is
/// already running starts nothing and is not counted. Once reached, the chat
/// is paused: nothing more is sent until the person writes.
pub const MAX_UNPROMPTED_STARTS: usize = 6;

/// Messages the agents of one session may exchange in all.
pub const MAX_SESSION_MESSAGES: usize = 60;

/// Workers answering a chat message at once. These jobs are apart from the
/// orchestrator's own ([`MAX_RUNNING_JOBS`]): a busy chat must not leave it
/// unable to delegate.
pub const MAX_CHAT_JOBS: usize = 3;

#[derive(Default)]
pub(super) struct Room {
    lead: Option<SessionActor>,
    messages: Vec<Message>,
    draining: HashSet<String>,
    pub(super) closing: HashSet<String>,
    /// Deliveries that started a turn or a job since the person last wrote.
    unprompted: usize,
}

impl Room {
    fn stopped(&self) -> Option<ChatStop> {
        if self.messages.len() >= MAX_SESSION_MESSAGES {
            Some(ChatStop::Full)
        } else if self.unprompted >= MAX_UNPROMPTED_STARTS {
            Some(ChatStop::Paused)
        } else {
            None
        }
    }
}

/// Why a session's agents can send no more.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatStop {
    /// Too many deliveries in a row started work by themselves; it resumes
    /// when the person writes in the session.
    Paused,
    /// The session's total is spent.
    Full,
}

impl ChatStop {
    /// What the sender is told.
    fn refusal(self) -> String {
        match self {
            Self::Paused => format!(
                "Team chat is paused: {MAX_UNPROMPTED_STARTS} messages in a row started work without the person writing anything. It resumes when they next write in this session. Until then, say what you have in your report."
            ),
            Self::Full => format!("This session's agents have exchanged {MAX_SESSION_MESSAGES} chat messages, the most one session allows. Say what is left in your report."),
        }
    }
}

/// A session's chat as the interface shows it.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatView {
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stopped: Option<ChatStop>,
    pub unprompted_limit: usize,
    pub message_limit: usize,
}

/// Something the host is told about a chat, as it happens.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ChatEvent {
    /// The session's chat changed: a message was sent, delivered or given up,
    /// or the chat was paused or resumed.
    Changed { session: String },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: usize,
    pub from: String,
    pub to: String,
    pub from_name: String,
    pub to_name: String,
    pub text: String,
    pub reply_to: Option<usize>,
    pub sent_at: u64,
    pub delivered: bool,
    pub error: Option<String>,
}

impl Delegation {
    pub fn attach_chat_lead(&self, actor: &SessionActor) {
        if let Some(team) = self.state().orchestrators.get_mut(&actor.handle().id) {
            team.chat.lead = Some(actor.clone());
        }
    }

    pub fn chat_history(&self, session: &str) -> Vec<Message> {
        self.state()
            .orchestrators
            .get(session)
            .map(|t| t.chat.messages.clone())
            .unwrap_or_default()
    }

    /// The session's messages, and whether its agents may still send.
    pub fn chat_view(&self, session: &str) -> ChatView {
        let state = self.state();
        let room = state.orchestrators.get(session).map(|team| &team.chat);
        ChatView {
            messages: room.map(|room| room.messages.clone()).unwrap_or_default(),
            stopped: room.and_then(Room::stopped),
            unprompted_limit: MAX_UNPROMPTED_STARTS,
            message_limit: MAX_SESSION_MESSAGES,
        }
    }

    /// Hears every chat event, for the interface. One listener: the host's.
    pub fn on_chat(&self, listener: impl Fn(ChatEvent) + Send + Sync + 'static) {
        *self.inner.chat_listener.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(listener));
    }

    /// Never called with the state locked: the listener may ask for it.
    fn chat_event(&self, event: ChatEvent) {
        let listener = self.inner.chat_listener.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
        if let Some(listener) = listener {
            listener(event);
        }
    }

    /// The person wrote in a session: its own, or one of its workers'. The
    /// agents of that team may start work for each other again.
    pub fn note_person(&self, handle: &str) {
        let session = {
            let mut state = self.state();
            let session = if state.orchestrators.contains_key(handle) {
                Some(handle.to_string())
            } else {
                state.jobs.values().find(|record| record.actor.is_some() && record.view.worker_session.as_deref() == Some(handle)).map(|record| record.view.orchestrator.clone())
            };
            match session.and_then(|session| state.orchestrators.get_mut(&session).map(|team| (session, team))) {
                Some((session, team)) if team.chat.unprompted > 0 => {
                    team.chat.unprompted = 0;
                    session
                }
                _ => return,
            }
        };
        self.chat_event(ChatEvent::Changed { session });
    }

    /// Counts a delivery that is about to start a turn or a job, and refuses
    /// it once the agents have started that many in a row by themselves.
    fn reserve_start(&self, session: &str, generation: &str) -> Result<(), String> {
        let mut state = self.state();
        let team = state.orchestrators.get_mut(session).filter(|team| team.token == generation).ok_or("Session ended.")?;
        if team.chat.unprompted >= MAX_UNPROMPTED_STARTS {
            return Err(ChatStop::Paused.refusal());
        }
        team.chat.unprompted += 1;
        Ok(())
    }

    /// Gives back a start that did not happen after all.
    fn release_start(&self, session: &str) {
        if let Some(team) = self.state().orchestrators.get_mut(session) {
            team.chat.unprompted = team.chat.unprompted.saturating_sub(1);
        }
    }

    /// Whether the team lets this caller use the chat at all
    /// ([`AgentChat`](crate::delegation::AgentChat)). A caller it leaves out
    /// is not offered the tools.
    pub fn chat_open_to(&self, caller: &Caller) -> bool {
        self.state().orchestrators.get(caller.orchestrator()).is_some_and(|team| team.combo.agent_chat.includes(caller.provider()))
    }

    /// The participants the caller may write to: the members the team's
    /// setting lets into the chat.
    pub fn chat_members(&self, caller: &Caller) -> Vec<Value> {
        let state = self.state();
        let Some(team) = state.orchestrators.get(caller.orchestrator()) else { return Vec::new() };
        let open = team.combo.agent_chat;
        let mut members = Vec::new();
        if open.includes(team.provider) {
            members.push(json!({"address": LEAD, "name": "Orchestrator", "provider": team.provider.as_str()}));
        }
        for id in state.order.get(caller.orchestrator()).into_iter().flatten() {
            if let Some(job) = state.jobs.get(id)
                && job.address == *id
                && open.includes(job.slot.provider)
            {
                members.push(json!({"address": id, "name": job.slot.name, "provider": job.slot.provider.as_str()}));
            }
        }
        members
    }

    pub fn send_chat(&self, caller: &Caller, to: &str, text: &str, reply_to: Option<usize>) -> Result<Message, String> {
        let session = caller.orchestrator();
        let text = text.trim();
        if text.is_empty() || text.len() > 32_000 {
            return Err("A message needs between 1 and 32000 bytes.".into());
        }
        let (message, start, generation) = {
            let mut state = self.state();
            let worker = |id: &str| state.jobs.get(id).filter(|job| job.view.orchestrator == session);
            let (lead, open) = state.orchestrators.get(session).map(|team| (team.provider, team.combo.agent_chat)).ok_or("Session ended.")?;
            let (from, from_name) = match caller {
                Caller::Orchestrator { .. } => (LEAD.to_string(), "Orchestrator".into()),
                Caller::Worker { job_id, .. } => {
                    let job = worker(job_id).ok_or("Unknown sender.")?;
                    (job.address.clone(), job.slot.name.clone())
                }
            };
            if !open.includes(caller.provider()) {
                return Err(CHAT_CLOSED.into());
            }
            let (to, to_name, to_provider) = if to == LEAD {
                (LEAD.to_string(), "Orchestrator".to_string(), lead)
            } else {
                let job = worker(to).ok_or("Unknown address. Use chat_members for this session's addresses.")?;
                (job.address.clone(), job.slot.name.clone(), job.slot.provider)
            };
            if from == to {
                return Err("Choose another member of the session.".into());
            }
            if !open.includes(to_provider) {
                return Err(format!("{to_name} does not take part in this team's chat. Say it in your report to the orchestrator instead."));
            }
            let team = state
                .orchestrators
                .get_mut(session)
                .filter(|team| team.root == caller.root())
                .ok_or("Session ended.")?;
            let room = &mut team.chat;
            if let Some(stop) = room.stopped() {
                return Err(stop.refusal());
            }
            if let Some(parent) = reply_to
                && !room.messages.iter().any(|m| m.id == parent && m.from == to && m.to == from)
            {
                return Err("Reply to a message from this recipient.".into());
            }
            let message = Message {
                id: room.messages.len() + 1,
                from,
                from_name,
                to: to.clone(),
                to_name,
                text: text.into(),
                reply_to,
                sent_at: now_unix_ms(),
                delivered: false,
                error: None,
            };
            room.messages.push(message.clone());
            (message, room.draining.insert(to), team.token.clone())
        };
        self.chat_event(ChatEvent::Changed { session: session.to_owned() });
        if start {
            let service = self.clone();
            let session = session.to_owned();
            let to = message.to.clone();
            tokio::spawn(async move { service.drain_chat(&session, &to, &generation).await });
        }
        Ok(message)
    }

    async fn drain_chat(&self, session: &str, to: &str, generation: &str) {
        loop {
            let message = {
                let mut state = self.state();
                let Some(team) = state.orchestrators.get_mut(session).filter(|team| team.token == generation) else {
                    return;
                };
                let Some(message) = team
                    .chat
                    .messages
                    .iter()
                    .find(|m| m.to == to && !m.delivered && m.error.is_none())
                    .cloned()
                else {
                    team.chat.draining.remove(to);
                    return;
                };
                message
            };
            let result = self.deliver_chat(session, &message, generation).await;
            {
                let mut state = self.state();
                if let Some(item) = state
                    .orchestrators
                    .get_mut(session)
                    .filter(|team| team.token == generation)
                    .and_then(|t| t.chat.messages.get_mut(message.id - 1))
                {
                    match result {
                        Ok(()) => item.delivered = true,
                        // Taken after all, by a turn that was already on its way.
                        Err(_) if item.delivered => {}
                        Err(error) => item.error = Some(error),
                    }
                }
            }
            self.chat_event(ChatEvent::Changed { session: session.to_owned() });
        }
    }

    async fn deliver_chat(&self, session: &str, message: &Message, generation: &str) -> Result<(), String> {
        let prompt = format!(
            "ThingMaker team chat #{} from {} ({}). To reply, use chat_send with to={} and reply_to={}.\n\n{}",
            message.id, message.from_name, message.from, message.from, message.id, message.text
        );
        let mut changed = self.inner.changed.subscribe();
        loop {
            let (actor, job) = {
                let state = self.state();
                let team = state
                    .orchestrators
                    .get(session)
                    .filter(|team| team.token == generation)
                    .ok_or("Session ended.")?;
                // The team's setting is read again here: it may have been
                // turned off since the message was sent.
                let open = team.combo.agent_chat;
                if team.chat.closing.contains(&message.to) {
                    (None, None)
                } else if message.to == LEAD {
                    if !open.includes(team.provider) {
                        return Err(CHAT_CLOSED.into());
                    }
                    (Some(team.chat.lead.clone().ok_or("Lead is not attached.")?), None)
                } else {
                    let record = state
                        .order
                        .get(session)
                        .into_iter()
                        .flatten()
                        .rev()
                        .filter_map(|id| state.jobs.get(id))
                        .find(|r| r.address == message.to)
                        .ok_or("Worker is gone.")?;
                    if !open.includes(record.slot.provider) {
                        return Err(CHAT_CLOSED.into());
                    }
                    (record.actor.clone(), Some(record.view.clone()))
                }
            };
            let mut active = false;
            if let Some(actor) = actor {
                let snapshot = actor.snapshot().await.map_err(|e| e.message)?;
                active = snapshot.foreground.is_active();
                if active {
                    if snapshot.capabilities.is_some_and(|caps| caps.supports_steering) {
                        match actor.steer_text(&prompt).await.map_err(|e| e.message)? {
                            SteerOutcome::Accepted { .. } => return Ok(()),
                            SteerOutcome::OutcomeUnknown { error } => return Err(error.message),
                            SteerOutcome::Rejected { error } => {
                                if actor.snapshot().await.map_err(|e| e.message)?.foreground.is_active() {
                                    return Err(error.message);
                                }
                            }
                        }
                    }
                } else if job.is_none() {
                    // An idle orchestrator: this message starts its next turn.
                    self.reserve_start(session, generation)?;
                    match actor.submit_text(format!("chat-{}", message.id), &prompt).await {
                        Ok(SubmissionOutcome::Accepted) => return Ok(()),
                        // It may have gone in: the start stays counted.
                        Ok(SubmissionOutcome::OutcomeUnknown { error }) => return Err(error.message),
                        Ok(SubmissionOutcome::Rejected { error }) => {
                            self.release_start(session);
                            if !actor.snapshot().await.map_err(|e| e.message)?.foreground.is_active() {
                                return Err(error.message);
                            }
                        }
                        Err(error) => {
                            self.release_start(session);
                            return Err(error.message);
                        }
                    }
                }
            }
            if let Some(job) = job
                && !active
                && job.status.is_finished()
            {
                // A worker with nothing running: this message is its next job.
                self.reserve_start(session, generation)?;
                let args = DelegateArgs {
                    task: prompt.clone(),
                    continue_job: Some(job.id),
                    pinned: true,
                    ..Default::default()
                };
                match self.delegate_inner(session, args, Some(message.id)) {
                    Ok(next) => {
                        let taken = self.await_chat_job(session, message, generation, &next.id, &mut changed).await;
                        if taken.is_err() {
                            self.release_start(session);
                        }
                        return taken;
                    }
                    Err(error) => {
                        self.release_start(session);
                        // A concurrent delegation can reserve the worker or the last free slot.
                        let state = self.state();
                        let jobs = || state.jobs.values().filter(|r| r.view.orchestrator == session);
                        if !state
                            .orchestrators
                            .get(session)
                            .is_some_and(|team| team.chat.closing.contains(&message.to))
                            && !jobs().any(|r| r.address == message.to && !r.view.status.is_finished())
                            && jobs().filter(|r| r.view.chat && r.view.status.is_active()).count() < MAX_CHAT_JOBS
                        {
                            return Err(error);
                        }
                    }
                }
            }
            tokio::select! { _ = changed.changed() => {}, _ = tokio::time::sleep(Duration::from_millis(300)) => {} }
        }
    }

    /// Waits for the job a message started to take it: delivered once the
    /// worker has the prompt, failed if the job ends first.
    async fn await_chat_job(&self, session: &str, message: &Message, generation: &str, job_id: &str, changed: &mut watch::Receiver<u64>) -> Result<(), String> {
        loop {
            {
                let state = self.state();
                let team = state.orchestrators.get(session).filter(|team| team.token == generation).ok_or("Session ended.")?;
                if team.chat.messages[message.id - 1].delivered {
                    return Ok(());
                }
                let record = state.jobs.get(job_id).ok_or("Job ended.")?;
                if record.view.status.is_finished() {
                    return Err(record.view.error.clone().unwrap_or_else(|| "Message was not accepted.".into()));
                }
            }
            changed.changed().await.map_err(|_| "Session ended.")?;
        }
    }

    pub(super) fn chat_accepted(&self, job_id: &str) {
        let mut state = self.state();
        let Some(job) = state.jobs.get(job_id) else { return };
        let Some(id) = job.chat_message else { return };
        let session = job.view.orchestrator.clone();
        if let Some(item) = state.orchestrators.get_mut(&session).and_then(|team| team.chat.messages.get_mut(id - 1)) {
            item.delivered = true;
            item.error = None;
        }
        self.bump();
    }

    pub(super) fn gemini_messages(&self, spec: &WorkerSpec, text: &str) {
        let caller = Caller::Worker {
            orchestrator: spec.orchestrator.clone(),
            root: spec.root.clone(),
            name: spec.slot.name.clone(),
            provider: Provider::Gemini,
            job_id: spec.job_id.clone(),
        };
        for line in text.lines().filter_map(|line| line.strip_prefix("TEAM_CHAT ")).take(16) {
            if let Ok(value) = serde_json::from_str::<Value>(line)
                && let (Some(to), Some(body)) = (value["to"].as_str(), value["text"].as_str())
            {
                let _ = self.send_chat(&caller, to, body, value["reply_to"].as_u64().and_then(|n| n.try_into().ok()));
            }
        }
    }
}
