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
