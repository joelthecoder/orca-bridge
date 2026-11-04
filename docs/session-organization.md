# Session organization controls

These controls require native Orca contracts. Stock Orca 1.4.242 does not advertise
them. The Bridge client refuses unsupported hosts before requesting a mutation;
it does not implement movement by closing, relaunching, changing directories, or
writing Orca databases. Native implementation and live acceptance remain pending.

## Transfer one whole terminal tab

```sh
orca-bridge terminals move HANDLE --worktree path:/absolute/destination --json
```

The handle identifies one selected pane; the operation transfers its **whole tab**,
including every split pane. `terminal.tab-transfer.v1` is required. Bridge obtains
the current incarnation and source workspace, then sends `terminal.transferTab`
with `terminal`, `destination`, `expectedIncarnationId`, `expectedWorktreeId`, and
`requestId`. The native owner must check these fences at commit time, refuse
cross-host transfer, and return its durable receipt. A moved tab keeps the actual
execution cwd/branch/host; its destination is organizational ownership.

An unconfirmed response includes its request ID in diagnostics. Inspect native
state, then reuse it with `--request-id ID`; do not issue a fresh move to recover
an ambiguous result. Identity, replay, rollback, and persistence are native
obligations, not guarantees conferred by a successful transport response.

## Preview, apply, and undo selected task names

Create a JSON array containing only the intended durable session IDs and names:

```json
[
  {"session":"local:codex:PROVIDER_ID","title":"Review terminal ownership"}
]
```

```sh
orca-bridge sessions names preview --stdin < selection.json
orca-bridge sessions names apply NATIVE_PLAN_ID --json
orca-bridge sessions names undo NATIVE_CHANGE_ID --json
```

`session.names.v1` is required. Preview resolves explicit IDs against one history
snapshot and forwards provider/host identities to `session.names.preview`. No
matching by title or cwd is permitted. A selection contains at most 100 unique
sessions. Names are nonempty, at most 512 UTF-8 bytes, and contain no control
characters. Unrecognized input fields are refused.

Manual names are preserved by default. `"replaceManual":true` requests a deliberate
replacement for that selected row. Inspect the native preview before applying.
The host owns the preview, expected revisions, conflicts, persistence, and undo.
Bridge forwards its `planId` to `session.names.apply` or `changeId` to
`session.names.undo`; it never replays a list of unconditional rename calls.
Inspect the returned conflict receipt: JSON transport success does not mean
that every selected name changed. Undo must preserve subsequent edits.

The existing `sessions rename` command remains a separate Bridge-owned display
alias; it does not gain native transaction semantics. Bulk controls do not write
that label store or rewrite provider transcripts.

## Names and status

Keep the task label stable. Native working, input, failure, and unknown evidence
are rendered separately from the name. An idle prompt, end of turn, unread badge,
or unreachable host is never verified task completion. Readers must not parse a
status prefix back into execution truth or persist it as the task's name.

## Verification boundary

Deterministic tests cover capability refusal before mutation, exact literal
arguments and identity fences, selected-session validation, native conflict
receipts, and absence of local label writes. They do not establish native process,
scrollback, restore, or rendered behavior. That acceptance requires a compatible
native build and disposable terminals in a separate background profile.
