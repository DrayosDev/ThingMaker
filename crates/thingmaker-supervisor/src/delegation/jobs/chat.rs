//! Session-scoped mailboxes. Addresses identify a conversation, even when two jobs use the same slot.
use super::*;
use crate::supervisor::SteerOutcome;
use serde_json::{Value, json};
use std::collections::HashSet;

const LEAD: &str = "orchestrator";

/// What a member the team's setting leaves out of the chat is told.
const CHAT_CLOSED: &str = "Agent chat is off for this member of the team. Say it in your report to the orchestrator instead.";

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
            let mut state = self.state();
            if let Some(item) = state
                .orchestrators
                .get_mut(session)
                .filter(|team| team.token == generation)
                .and_then(|t| t.chat.messages.get_mut(message.id - 1))
            {
                match result {
                    Ok(()) => item.delivered = true,
                    Err(error) => item.error = Some(error),
                }
            }
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
                    match actor
                        .submit_text(format!("chat-{}", message.id), &prompt)
                        .await
                        .map_err(|e| e.message)?
                    {
                        SubmissionOutcome::Accepted => return Ok(()),
                        SubmissionOutcome::OutcomeUnknown { error } => return Err(error.message),
                        SubmissionOutcome::Rejected { error } => {
                            if !actor.snapshot().await.map_err(|e| e.message)?.foreground.is_active() {
                                return Err(error.message);
                            }
                        }
                    }
                }
            }
            if let Some(job) = job
                && !active
                && job.status.is_finished()
            {
                let args = DelegateArgs {
                    task: prompt.clone(),
                    continue_job: Some(job.id),
                    pinned: true,
                    ..Default::default()
                };
                match self.delegate_inner(session, args, Some(message.id)) {
                    Ok(next) => loop {
                        {
                            let state = self.state();
                            let team = state
                                .orchestrators
                                .get(session)
                                .filter(|team| team.token == generation)
                                .ok_or("Session ended.")?;
                            if team.chat.messages[message.id - 1].delivered {
                                return Ok(());
                            }
                            let record = state.jobs.get(&next.id).ok_or("Job ended.")?;
                            if record.view.status.is_finished() {
                                return Err(record.view.error.clone().unwrap_or_else(|| "Message was not accepted.".into()));
                            }
                        }
                        changed.changed().await.map_err(|_| "Session ended.")?;
                    },
                    Err(error) => {
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

    pub(super) fn chat_accepted(&self, job_id: &str) {
        let mut state = self.state();
        let Some(job) = state.jobs.get(job_id) else { return };
        let Some(id) = job.chat_message else { return };
        let session = job.view.orchestrator.clone();
        if let Some(team) = state.orchestrators.get_mut(&session) {
            team.chat.messages[id - 1].delivered = true;
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
