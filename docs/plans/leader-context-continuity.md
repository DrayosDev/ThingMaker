# Context continuity when changing leaders

Changing providers creates a new native conversation. The new leader receives a
recent excerpt and can consult a private Markdown archive of the earlier visible
conversation through `team.read_continuity`. The archive is reference material:
the leader is told to read the pages it needs when the excerpt is not enough, not
to load all of it before starting, which for a long session would cost more than
a context window should be spent on. This applies to a provider change on send,
an explicit team handoff, and optional continuation after an exhausted account's
usage limit.

The archive is an extra and never a condition. When it cannot be made (the
conversation is over the size limit, the storage refused), cannot be bound, or
the new session has no way to read one (Local Chat, a session without the team
tools), the change of leader goes ahead with the excerpt alone, as it did before
archives existed.

## Settings and export

Settings → Continuity enables automatic archives by default. The same page can
export an open conversation as `.md` using the native save dialog. A continuation
shows its archive location and an export action above the composer. An export
saves no archive: it is the session's own conversation as it stands, behind the
ones it was handed, read back from their archives.

A paired phone can change provider, with the excerpt alone. The continuity
commands are not on the phone's list: saving one writes a document of the
caller's choosing into ThingMaker's data, and a phone writes no files. Nor can a
phone choose the account a session opens on, or prepare a continuation on
another account.

The archive includes visible messages, available tool inputs/results and patches,
plans, notices, team configuration, and known goal/job state. It excludes unsent
composer drafts, queued prompt text, thought messages, and embedded binary media.
Resource references are retained with explicit availability notes. History gaps,
provider-truncated content, and unavailable attachments are identified. Provider
internal state and hidden reasoning cannot be exported or transferred.

Local Chat has no tools, so a transfer to it carries the excerpt alone; a local
agent can read the archive. Turning the option off gives every transfer the
excerpt alone.

## Storage and authorization

Archives are immutable UUID-named Markdown files under the application's private
`continuity` data directory, outside the workspace. A separate SQLite index stores
ownership, BLAKE3 integrity hashes, and destination bindings. Authorization includes
the canonical workspace, provider, pinned account, and durable native conversation
id. An archive holds its own session's conversation and nothing else. It names
the archive that session was itself handed, and a reader follows that chain link
by link (up to 32 hand-offs back), including after restarting ThingMaker; nothing
is copied forward. Archive-reader tool outputs are represented by references in
the next archive for the same reason.

What is kept is bounded: 200 archives and 256 MiB in all, oldest deleted first,
and of a session's own archives that no hand-off completed with, only the newest
two. Archives are deleted with their sessions: Wipe All deletes those of the
sessions it deletes, and forgetting a workspace or deleting a quick chat deletes
all of theirs. Deleting an archive takes its words from every later reader, whose
chain simply ends there.

Renderer commands require a current live attachment. The team reader only exposes
the chain the calling leader was handed, without saying where the files are;
workers cannot read it. Offsets count Unicode code points, with 12,000 characters
per page by default and a maximum of 24,000. A conversation over 16 MiB is not
archived, and its hand-off carries the excerpt. Missing, modified, redirected, or
unauthorized files fail closed.

A failure preparing a transfer retains the source and its composer. An unused
destination is stopped. An ambiguous error after submit retains both sessions for
outbox reconciliation. The source sequence is checked again before submission;
new source events require retrying with a new snapshot. Metadata writes are
serialized so independent transfers cannot overwrite each other's references.
The record of continuations is a convenience: it keeps the newest 300, a prepared
brief waits in a preference of its own rather than inside it, and a write that
fails does not undo a change of leader.

Old development records without an account/workspace scope do not automatically
inject their pending private briefs. Recreate a handoff from the original chat if
one of those older prepared continuations still needs its context.

## Validation

Frontend coverage includes private export, omitted content, safe Markdown fences,
multiple handoff entry points, persistence, source changes during startup,
concurrent transfers, draft edits, archive/binding failures, and Local Chat.
Native coverage includes A → B → C across store reopenings, account and workspace
isolation, worker denial, Unicode pages, size limits, junction escape, immutable
bindings, and same-size content tampering.

Manual acceptance:

1. Open Settings → Continuity and leave automatic continuity enabled.
2. In a conversation, state a recognizable requirement, use a tool, and make a
   plan. Change its leader to another provider and send a continuation request.
3. Confirm the new leader reads the archive and recalls the early requirement.
   Check the continuity banner and export the document to inspect its contents.
4. Restart the app and change leaders again. Verify that the first conversation's
   requirement and tool results remain in the new export.
5. Turn automatic continuity off and verify the limited-excerpt behavior.

Automated coverage does not establish that every real model will obey the full
reading instruction or retain an arbitrarily long document in its context window.
The document remains available for rereading; missing information must be reported.
