# Cross-project routing

Bridge supplies session discovery, explicit worktree selection, literal prompt
delivery and Orca's launch receipt. Shared agent instructions decide when those
operations must be used. A second router app is unnecessary for that boundary.

`orca-bridge skills routing` prints this focused guide without Orca/HOME;
`orca-bridge skills routing --json` adds a machine-readable routing contract.
The main `skills`/`skill` command includes the same directive in its portable skill
and advertises this guide. Explicit user session-placement instructions win.

For implementation in a different project:

1. Discover the registered project and inspect the intended existing worktree.
2. Prepare a self-contained task: objective, exact repository/worktree, constraints,
   relevant context, expected checks and the result the coordinator needs back.
3. Use `sessions create --worktree SELECTOR --agent AGENT --prompt - --json`.
4. Inspect native `outcome` and `receipt`. A session launch, prompt delivery and
   finished implementation are distinct events. Preserve failures and uncertainty.
5. Leave target-project edits to that session. Report a failed handoff and continue
   independent coordinator work instead of silently doing the implementation here.

`sessions create` requires exactly one explicit `--worktree` or `--project`.
`--worktree` launches in an existing workspace. `--project` with `--name` creates a
new checkout and launches there. Empty and implicit `active`/`current` targets are
refused; no implicit caller-folder target is allowed.
Chat versus terminal follows Orca's configured surface; inspect `outcome.kind`.
Model, effort, accounts, approval and sandbox preferences remain Orca's responsibility.

For an ambiguous existing-worktree launch reply, inspect the selected workspace
and retry the same operation with `--retry-launch OPERATION_ID`. Never treat a lost
reply as permission for a fresh duplicate launch. Older runtime limitations and
native recovery errors remain visible to the caller.

The personal default lives in `~/.claude/AGENTS.md`, shared with Claude and Codex by
existing links. It permits coordinator read-only research and explicitly requested
shared workflow/configuration edits. Existing-session reuse needs the user's request
and exact identity verification. Explicit user session-placement instructions win.

For an explicitly selected existing terminal, both send aliases support
`--wait-submit SECONDS` to observe accepted prompt submission and `--retry-request
REQUEST_ID` to recover the original exact prompt after an ambiguous reply. These
require Enter submission and never resend on observation timeout. Use `--json-errors`
to retain typed outcome certainty and native recovery IDs; set `--command-timeout`
longer than the observation if needed. See [automation.md](automation.md). Output
arrival or a queued receipt is not task completion.

This policy is not a technical guarantee that an agent cannot ignore instructions.
Bridge cannot intercept filesystem edits made outside Bridge. Strict enforcement
would require a separately designed tool/write-permission guard that accounts for
legitimate cross-project research, worktrees and shared configuration. This change
does not install that guard, an automatic dispatcher or a second routing service.

## Handoff template

```text
Objective: <the concrete change or result>
Target: <explicit repository/worktree and branch>
Authorized scope: <files/areas this worker owns; preserve unrelated work>
Context: <relevant decisions, source links and current evidence>
Constraints: <behavior, compatibility, approval and publication limits>
Validation: <project Make targets and behavioral checks>
Return: <what changed, evidence/check results, remaining limits and commit if authorized>
```

Pipe the completed text as literal stdin, inspect native `outcome` and `receipt`,
and only then report the observed handoff state. Do not say the target task is done
because a launch succeeded. Use exact returned handles/incarnations for terminal
read/follow and provider IDs for history; a title/path match does not prove ownership.
If a launched Chat surface cannot be observed through available APIs, report launch
acceptance and the observation gap rather than asserting prompt processing.
