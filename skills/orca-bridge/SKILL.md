---
name: orca-bridge
description: Use Orca Bridge to find Orca conversations, inspect UI-ready workspace data, send requested context to agents, manage Orca automations and project settings, or control explicitly selected sessions.
---

# Orca Bridge

Use the installed `orca-bridge`, or `~/.local/bin/orca-bridge`. Read
`orca-bridge help --json` for the current command tree; narrow it with
`help terminals send --json`. `orca-bridge agent` prints the output and identity
contracts. Discovery works without the app. `doctor --json` reports actual availability.

## Find and inspect

```sh
orca-bridge recent --limit 10 --json
orca-bridge search 'topic' --agent codex --json
orca-bridge sessions show ID --usage --json
orca-bridge snapshot --limit 100 --json
orca-bridge schema session
orca-bridge schema snapshot
```

Search reads the local index without pairing or a running app. Full-text search uses
`search QUERY --full-text`. Snapshot supplies separate projects, repositories,
worktrees, terminals and tabs, plus history sessions with workspace contexts and
proven live bindings. Check each resource's `available`, and both truncation fields.
A matching directory links location metadata; it never proves live ownership.

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
cat context.md | orca-bridge terminals send HANDLE --text - --enter
printf 'Review this project' | orca-bridge sessions create --worktree path:/absolute/project --agent codex --prompt -
```

Use `--agent claude` for Claude. Launches follow Orca's configured chat/terminal
surface, accounts, approval and sandbox settings. Do not add approval or sandbox
flags. Omit `--enter` to stage terminal input for review. Send is literal text,
not shell evaluation. Native Chat UI messages have different IDs/APIs; terminal
send does not imply complete native-chat control.

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

## Automations and projects

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

`watch` polls local state; `stream subscribe` uses separately paired native push.
Use bounded `--count`/`--timeout` for inspection. Consumers of long-lived streams must
reconcile snapshots/cursors after reconnect. Authentication and rejected requests
fail rather than retrying indefinitely. Cancelling a follower leaves agents running.

Pipes default to JSON; streams always emit JSONL. stdout is data, stderr diagnostics.
Exit 0 means success or downstream broken pipe; 1 means runtime/input error; 2 means
CLI syntax error. Null model/status/usage means unknown, not idle or zero.
Never print private pairing links or read/export credential files. Raw `rpc` and
`orca --` can mutate the runtime; their effect depends on the method/command.
