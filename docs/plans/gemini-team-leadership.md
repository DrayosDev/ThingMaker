# Gemini ordinary team leadership

A Gemini session that leads a team uses the official `agy acp` JSON-RPC
transport, the only mode that takes the session's own `team` server. Every other
Gemini session keeps the stream-json bridge: one opened without a team, every
conversation from before, and every Gemini worker.

A new session is opened natively when it starts with at least one helper. The
transport is recorded with the session, and a reopened session keeps the one it
began on, so a conversation never moves between the two. A session on the bridge
cannot be given a team while it runs; the interface keeps the team and continues
the next message in a new Gemini session that leads it, as a change of provider
would. A Gemini worker gets no team server: it takes part in the agent chat
through `TEAM_CHAT` lines in its replies, and the server withholds the chat tools
from it, so a member has one way into the chat and the team's agent chat setting
decides for both.

Native sessions pass MCP entries unchanged through `session/new` and
`session/load`, including the per-attachment relay token. They never install a
global team server or write shared Gemini settings. `--process-per-session` is
included for session isolation and editor-provided HTTP MCP compatibility.
Additional accounts retain their existing private home, explicit `--gemini-dir`
and relative `--app-data-dir antigravity-cli`; credential guards and the verified
file-auth version gate remain unchanged. New CLI versions are not automatically
approved for additional-account authentication.

Model, effort and permission mode are applied through the existing actor's
session configuration flow. ACP permission callbacks obey the workspace stance.
Guidance is an ordinary prefix block on the first prompt of each attachment,
including resume. No unverified system-prompt metadata or launch flags are sent.
When native subagents are withheld, guidance directs Gemini to use only the
selected team; there is currently no verified hard-disable hook for native
Gemini subagents. A failed load or a different returned conversation ID fails
attachment rather than silently starting a replacement conversation.

Big Thing policy is separate: `Provider::can_lead_goal` retains its previous
provider set and Gemini goal start/move is rejected. The saved-team provider list
includes Gemini; Big Thing's orchestrator schema is unchanged.

Official ACP reference: https://antigravity.google/docs/ide/extensions/acp/ .
The documented native resume does not replay history. Provider context stays in
the existing conversation, but replay of old messages into a freshly attached
desktop panel requires separate transcript support. The existing additional
account file-auth gate is pinned to 1.5.9; adding another version requires its own
isolation validation.

Focused verification: `cargo test -p thingmaker-supervisor --test gemini_leader
--locked --offline`. The Node ACP fixture launches the actual MCP relay and
exercises configuration, isolated tokens, permission callbacks, delegation,
worker reports, resume and failure cleanup. Node must be on PATH or supplied in
`THINGMAKER_TEST_NODE`; missing Node fails rather than silently skipping tests.
Existing stream-json compatibility remains covered by `--test gemini_peer`.
Live model calls are intentionally excluded from these tests.

CLI model variants such as `gemini-4-neon-high` are translated to the advertised
native model `gemini-4-neon` and `thought_level=high`. Exact advertised model IDs
take precedence over suffix parsing. The same translation applies when a saved
team changes the model of an existing Gemini session.

## Explicit continuation after exhaustion

A session's own rejection or explicit exhaustion error offers an optional
continuation. Unknown usage and generic throttling do not trigger the offer.
The user selects a signed-in account or provider and prepares a fresh session;
opening it never submits a prompt or changes the provider's global account.
The native request pins `accountId`, and snapshots report the actual pinned
account. Invalid accounts fail without selecting another identity. The pin is
kept with the session: a later reopening, after a restart too, runs on it again,
its workers on the same provider run on it, and a copy of the session inherits
it. Asking for the account that is active is not a pin. Once a pinned account
has been removed the session follows the active one again.

The destination receives the team, editable draft, attachments and mentions.
Its first message carries a recent-conversation summary with a 24,000-character
text budget. The summary waits in a preference of its own until a send is accepted.
The original session and composer remain available. Jobs, background activity,
pending messages and active Big Thing records block this chat-only handoff.
Provider-wide readings are used only for sessions on the corresponding active
account; another account's own stream is used when available.

The opt-in real-provider test is
`a_real_gemini_leader_delegates_reads_the_worker_report_and_resumes`: one real
Gemini leader turn with a deterministic simulated worker, then the same
conversation reopened through `session/load` for a second turn that has to
remember the worker's report. It requires `THINGMAKER_REAL_GEMINI=1` and
`THINGMAKER_AGY`, and is ignored by default. `tests/real_continuity.rs` hands a
conversation to a real Codex leader, which has to fetch a fact from the archive.
