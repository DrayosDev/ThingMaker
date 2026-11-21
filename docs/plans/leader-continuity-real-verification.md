# Real-provider continuity verification — 2026-11-21

## Method

An isolated Windows instance ran the compiled desktop backend with its real
WebView/Tauri IPC, authenticated providers, native team server and HostLauncher.
A temporary verification bundle loaded the production `store.ts` and `ipc.ts`
implementations into that WebView; it used a separate Zustand store to drive the
same handoff/send methods. No provider, worker, response, archive or IPC call was
mocked. This is a real integration test, rather than a mouse-driven UI test.

Only a disposable workspace and verification conversations were used. Sentences
containing artificial project requirements were followed by explicitly synthetic
padding to move those requirements outside the bounded handoff excerpt. The
captured destination prompts were checked not to contain the original control
token. Successful replies alone were insufficient: archive-reader calls, page
offsets, terminal pages and actual worker job results were inspected.

## Results

| Case | Result | Evidence |
| --- | --- | --- |
| Gemini leader → real Gemini worker | Passed | `list_workers`, `delegate`, `await_jobs`; successful native worker result. |
| Gemini → Claude | Blocked by authentication | New leader, team and archive binding were prepared, but Claude's first turn returned `401 OAuth access token has expired`. No successful Claude context-recovery claim. |
| Failed Claude session → Codex, retaining the original Gemini conversation | Passed | Codex recovered the original token, retry count, output format, pending action and first worker result. Full 45,095-code-point archive read through its terminal page; real Gemini worker completed the next task. |
| Complete verification-app shutdown/restart → resumed Codex | Passed | The original archive binding survived without being recreated. Codex rediscovered and read its 45,095-code-point archive and recovered the original facts. |
| Resumed Codex → Gemini | Passed | Five contiguous pages covered all 88,438 code points. Gemini recovered the original facts and delegated successfully to a real Codex worker using the configured name, model and effort. |
| Explicit `handOff`, Gemini → Codex, with corrected job capture | Passed | Archive contained the latest successful native Codex worker result despite an empty frontend jobs cache. Codex read all 103,296 code points and recovered that result plus the original token and pending action. |

All successful checks used the same workspace files. No requirements were stored
in workspace files or shared memory, preventing those sources from supplying the
control answers instead of the archive. Team selection was preserved through the
tested handoffs. The reverse test explicitly selected a Codex worker before
transferring leadership and verified that selection survived the transfer.

## Fixes and preparation discovered by these tests

- Continuity capture now fetches authoritative native jobs and goal state instead
  of relying on a possibly stale frontend cache. Failure to obtain them aborts
  archive creation. A regression test covers a completed result absent from the
  cache.
- Individual tools whose outputs were not exposed in the transcript now carry an
  explicit availability explanation. Delegation results also appear in the Jobs
  section. No missing raw result is reconstructed or invented.
- The isolated test environment initially selected Codex's default profile, whose
  SQLite state returned error 522. Explicitly selecting an already authenticated
  working profile through ThingMaker's runtime-environment setting enabled the
  real Codex checks; credentials were not copied.
- Claude's adapter was absent in the isolated environment. The repository-tested
  ACP adapter version 0.89.1 was installed in the ignored verification directory
  and selected there. The actual first turn then exposed the expired OAuth token
  despite the CLI's status check reporting a signed-in account. Completing Claude
  verification requires usable Claude access. The user subsequently confirmed
  that no Claude access was available for these tests, so Claude remains unverified.

## Scope and limitations

Gemini's ACP tool updates did not expose every raw tool result. For pagination,
the test therefore correlated its actual input offsets with the corresponding
native output artifacts of this verification conversation; all five pages were
successful and contiguous. This does not justify weakening the tool's strict
argument validation. An initially suspected extra-argument issue was disproved;
no such compatibility change was made.

These checks establish the recorded cases, not perfect recall of arbitrarily
large history or transfer of hidden reasoning, binary attachments, internal
provider state, or tool outputs the provider does not expose. Claude recovery,
Claude-led delegation, and the remaining Claude transfer directions are pending
reauthentication. Actual usage exhaustion was not deliberately induced.

Local ignored evidence is under `output/continuity-real-*-evidence.json`,
`output/continuity-real-gemini-pages.json`, and the verification/build logs.
