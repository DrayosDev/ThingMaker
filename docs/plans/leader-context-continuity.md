# Context continuity when changing leaders

Changing providers creates a new native conversation. The new leader receives a
recent excerpt and can read a private Markdown archive of the available visible
conversation through `team.read_continuity`. It is instructed to read from offset
zero, following `nextOffset` until it is null, before resuming transferred work.
This applies to a provider change on send, an explicit team handoff, and optional
continuation after an exhausted account's usage limit.

## Settings and export

Settings → Continuity enables automatic archives by default. The same page can
export an open conversation as `.md` using the native save dialog. A continuation
shows its archive location and an export action above the composer. Paired remote
clients can prepare transfers; desktop file dialogs remain on the computer.

The archive includes visible messages, available tool inputs/results and patches,
plans, notices, team configuration, and known goal/job state. It excludes unsent
composer drafts, queued prompt text, thought messages, and embedded binary media.
Resource references are retained with explicit availability notes. History gaps,
provider-truncated content, and unavailable attachments are identified. Provider
internal state and hidden reasoning cannot be exported or transferred.

Local Chat has no tools. With automatic continuity enabled, a transfer to Local
Chat is refused with an explanation; a local agent can read the archive. Turning
the option off keeps the previous limited-excerpt behavior.

## Storage and authorization

Archives are immutable UUID-named Markdown files under the application's private
`continuity` data directory, outside the workspace. A separate SQLite index stores
ownership, BLAKE3 integrity hashes, and destination bindings. Authorization includes
the canonical workspace, provider, pinned account, and durable native conversation
id. A destination inherits its predecessor's complete archive when it creates its
next snapshot, including after restarting ThingMaker. Archive-reader tool outputs
are represented by references in the next snapshot to avoid copying inherited
history twice.

Renderer commands require a current live attachment. The team reader only exposes
the calling leader's bound archive; workers cannot read it. Offsets count Unicode
code points, with 12,000 characters per page by default and a maximum of 24,000.
Documents exceeding 16 MiB are rejected explicitly rather than truncated. Missing,
modified, redirected, or unauthorized files fail closed.

A failure preparing a transfer retains the source and its composer. An unused
destination is stopped. An ambiguous error after submit retains both sessions for
outbox reconciliation. The source sequence is checked again before submission;
new source events require retrying with a new snapshot. Metadata writes are
serialized so independent transfers cannot overwrite each other's references.

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
