---
name: orca-bridge
description: Use Orca Bridge to find Orca conversations, inspect UI-ready workspace data, send requested context to agents, manage Orca automations and project settings, or control explicitly selected sessions.
---

# Orca Bridge

Use the installed `orca-bridge`, or `~/.local/bin/orca-bridge`. Read
`orca-bridge help --json` for the current command tree; narrow it with
`help terminals send --json`. `orca-bridge agent` prints the output and identity
contracts. Discovery works without the app. `doctor --json` reports actual availability.

For routing requests, read `orca-bridge skills routing` (or `--json` for its explicit
routing contract). Treat cross-project implementation as a handoff obligation unless
the user's session-placement instructions explicitly override it.

## Find and inspect

```sh
orca-bridge recent --limit 10 --json
orca-bridge search 'topic' --agent codex --json
orca-bridge sessions show ID --usage --json
orca-bridge sessions messages ID --role assistant --limit 5 --json
orca-bridge sessions transcript ID --last 20
orca-bridge snapshot --limit 100 --json
orca-bridge schema session
orca-bridge schema snapshot
```

Search reads the local index without pairing or a running app. Full-text search uses
`search QUERY --full-text`. Snapshot supplies separate projects, repositories,
worktrees, terminals and tabs, plus history sessions with workspace contexts and
proven live bindings. Check each resource's `available`, and both truncation fields.
A matching directory links location metadata; it never proves live ownership.

Use `sessions messages` for bounded recent Codex/Claude user/assistant text without
tools or thinking. Check `sourceTruncated`, `limitTruncated` and
`incompleteFinalRecord`; an empty bounded result is not proof of no history. Default
source tail is 2 MiB (`--max-bytes`, maximum 16 MiB). Source phase/timestamp can be
unknown. Other providers fail explicitly. `transcript --last N` returns recent raw
records and can combine with `--follow`; `--tail` starts at EOF.

## Navigate, send context and launch

`open ID` focuses a proven live conversation, or resumes only when closure is proven.
Unknown ownership fails. `sessions resume ID` explicitly creates a new historical
resume. Do not match titles or folder names to choose an agent to control.

### Clickable targets in Orca terminals

Use `sessions link ID --terminal --json` for a freshly proven native terminal target.
Show the task name followed by the returned visible `term_…` handle. Orca makes that
handle clickable (Cmd-click on macOS, Ctrl-click elsewhere). Do not hide it behind
a Markdown label. Unknown or multiple live bindings fail; never guess from cwd/title.
Handles expire across runtime changes, so reacquire them before reuse.

`sessions link ID --json` still returns a durable `orca-bridge://session/…` URL for
applications that support custom URL schemes. A registered OS handler does not make
that URL clickable in Orca terminal output. When asked to go to the tab, use `open ID`
and verify `action: focused` and `navigated: true` instead of returning another link.

```sh
orca-bridge terminals list --json
orca-bridge terminals read HANDLE --tail 12 --json
cat context.md | orca-bridge terminals send HANDLE --text - --enter
printf 'Review this project' | orca-bridge sessions create --worktree path:/absolute/project --agent codex --prompt -
```

Use `--agent claude` for Claude. Launches follow Orca's configured chat/terminal
surface, accounts, approval and sandbox settings. Do not add approval or sandbox
flags. Omit `--enter` to stage terminal input for review. Send is literal text,
not shell evaluation. Native Chat UI messages have different IDs/APIs; terminal
send does not imply complete native-chat control.

`read --tail N` preserves draft/cursors/metadata and adds `bridgeTail` trimming
metadata. Cursors are opaque: pass `terminal.nextCursor` to `terminals follow HANDLE
--cursor CURSOR --count 1 --timeout 30` to wait for changed output. Unchanged cursors
emit nothing; timeout completes normally. Output arrival does not prove completion.

Both send aliases expose `--wait-submit SECONDS` and `--retry-request REQUEST_ID`
with `--enter`. Observation timeout never resends. Replay requires the original
exact prompt and incarnation; inspect receipts and host support rather than guessing.

Session IDs are durable provider identities; terminal handles are runtime identities.
Reacquire handles after restart/incarnation changes. `sessions rename ID TITLE` sets
a persistent Bridge display title; it preserves originalTitle and provider history.
`terminals rename HANDLE TITLE` requests a live-tab title change; check its verified
field rather than treating acknowledgement as persistence. `sessions stop ID`
or `terminals stop HANDLE` targets an exact PTY and preserves history. `terminals close
HANDLE --tab` removes the selected tab. Choose the authorized target explicitly; never
clean up a user's other sessions merely because they look idle.

## Selected organization controls

`terminals move HANDLE --worktree SELECTOR` requests a same-host whole-tab transfer,
including all split panes, through the native owner. It requires
`terminal.tab-transfer.v1`; stock Orca 1.4.242 does not advertise it. Never simulate
the operation with a close, relaunch, resume, `cd`, or direct database write.
An ambiguous reply names a request ID; inspect state and reuse `--request-id ID`.

`sessions names preview --stdin` accepts an explicit JSON array of
`{session,title,replaceManual?}` (at most 100 unique durable session IDs).
Inspect the native preview, then use `sessions names apply PLAN_ID`.
`sessions names undo CHANGE_ID` requests conflict-aware undo. These require
`session.names.v1`, not yet advertised by stock Orca 1.4.242. Native receipts own
conflicts; transport success does not establish that every name changed. Manual
names are preserved unless replacement was explicitly requested for that row.
These controls never write the Bridge's separate display-alias store.

Keep task names stable and display native status separately. Idle/end-of-turn is
never verified completion; missing/stale evidence remains unknown.

## Resource inspection and manual cleanup

Use `resources --limit 20 --json` for current app/host and terminal process-tree
memory/CPU; `resources --terminal HANDLE` selects one verified terminal binding.
`schema resources` defines the schema. RSS is bytes, CPU is percent, null is unknown,
and estimates are not guaranteed reclaimable memory. This is an on-demand diagnostic
sweep. Keep unbound rows unknown; do not turn diagnostic PIDs into arbitrary kills.

`worktrees ps --json` gives live/attached/unverifiable counts without prompts.
`worktrees usage --limit 20 --timeout 15 --json` inspects bounded local allocated disk
and Git-change estimates; `--worktree SELECTOR` targets one. Check all truncation,
availability and host-scope fields before choosing cleanup.

Use `worktrees inventory --include-external --json` to inspect native archived state
plus Git entries omitted by Orca's list, scoped only to registered repositories.
Detached HEAD, locked, prunable and missing-path states are separate facts with
provenance/coverage; none means safe to delete. `git-discovered` is not proof of being
unregistered. Copy only verified native selectors, then preview. This never prunes
Git metadata, changes visibility or scans arbitrary filesystem locations.

`terminals stop HANDLE --expected-incarnation ID` preserves history and refuses a
changed target. `worktrees remove SELECTOR` deletes a stopped local linked worktree
through Orca and may delete its merged local branch. Main/base and live/unverifiable
worktrees are refused. `--force` only permits discarding dirty/untracked work.
Archive hooks run only with `--run-hooks`; `--allow-failed-archive-hook` is a separate
override. Never stop or delete other work merely because usage is low or it looks idle.

Preview first with `worktrees remove SELECTOR --dry-run --json`. Inspect `allowed`,
blockers and effects; exit 0 only means inspection succeeded. Preview runs no hooks
or deletion; actual removal repeats checks and returns acknowledged vs verified/
unknown read-back. Never replay removal with its local correlation ID. Inspection
includes exact selectors. Resources support `--repo`/`--worktree` and memory/CPU sort;
usage supports `--repo`. Preserve coverage and global-versus-selected totals.

## Route implementation to the target project

1. Decide whether the implementation belongs to the current project. If it belongs
   elsewhere, keep this session as coordinator. An in-process subagent rooted in the
   current project does not count as routing to the other project.
2. Discover the registered repository/worktree and run `worktrees show SELECTOR`
   before dispatch. Select the exact target; never infer ownership from a title or
   silently default to the caller's directory. Ask only if the actual target remains
   ambiguous after discovery.
3. Pipe a self-contained task into `sessions create --worktree SELECTOR --agent AGENT
   --prompt - --json --json-errors`. Include the objective, target, constraints,
   authorized file scope, validation, context and required result. Use `--project`
   plus `--name` only when a new checkout is intended. Do not add approval/sandbox flags.
4. Inspect native `outcome` and `receipt` before reporting a handoff. Launch acceptance,
   observed prompt delivery and task completion are different facts. For terminal
   observation use the exact returned handle/incarnation; Chat UI uses its own APIs.
   If response observation is unsupported, report that limitation instead of inventing
   receipt or completion evidence.
5. On an ambiguous launch reply, inspect and retry the original operation with
   `--retry-launch`; never create a fresh duplicate. An explicitly requested existing
   terminal handoff similarly requires exact identity and original-request replay
   (`--retry-request`, `--wait-submit`) without changing its payload.
6. If Bridge cannot launch or verify the handoff, report the blocker and continue
   independent coordinator work. **Do not silently edit the target project here.**

Read-only cross-project research and explicitly requested shared workflow/config
edits can stay with the coordinator. Respect explicit user overrides. This policy
does not impose filesystem permissions or guarantee that another agent obeys it.

## Automations and project settings

Use `automations list|show|create|edit|run|runs|remove` for Orca's own automation system.
`automations create --help` explains schedules and stdin prompts. Orca validates
presets/cron/RRULE and fences ownership. `--disabled` is useful for a requested draft;
creating, enabling, running and removing are distinct user intents. Do not invent
schedules or run real tasks simply to test discovery.

`projects settings show SELECTOR --json` inspects a repository; `settings set SELECTOR
--updates JSON` patches only named fields. For artwork use `projects icon import` and
`projects icon apply`. Import preserves existing artwork; apply changes only the icon.
Use the actual help for accepted fields and PNG limits.

## Streams and contracts

Use `--json-errors` for stable codes/outcome certainty and safe native retry IDs on
stderr, with unchanged exit codes and clean stdout. Unknown is not proof that an
action did not happen. Native helper calls are bounded (default 60s/8MiB); configure
`--command-timeout`/`--max-output-bytes` when needed. Cancellation only stops Bridge's
owned helper, not a potentially accepted runtime action. Never blindly retry.

`doctor` distinguishes supported/unsupported/unknown runtime/CLI evidence without
mutating probes. `--fields` projects finite read JSON while retaining coverage and
blockers; mutations/streams reject it. `schema error`, `worktree-usage`, `removal-plan`
and `removal-receipt` expose contracts. `completions SHELL` generates offline scripts.

`watch` polls local state; `stream subscribe` uses separately paired native push.
Use bounded `--count`/`--timeout` for inspection. Consumers of long-lived streams must
reconcile snapshots/cursors after reconnect. Authentication and rejected requests
fail rather than retrying indefinitely. Cancelling a follower leaves agents running.

Pipes default to JSON; streams always emit JSONL. stdout is data, stderr diagnostics.
Exit 0 means success or downstream broken pipe; 1 means runtime/input error; 2 means
CLI syntax error. Null model/status/usage means unknown, not idle or zero.
Never print private pairing links or read/export credential files. Raw `rpc` and
`orca --` can mutate the runtime; their effect depends on the method/command.
