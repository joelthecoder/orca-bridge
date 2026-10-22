# Orca Bridge for agents

Orca Bridge is a local Rust CLI for finding Orca conversations, inspecting runtime
state, sending literal prompts, managing workspaces and following activity.

Start with `orca-bridge help --json` for the complete command tree, flags, defaults,
allowed values and effects. `orca-bridge help terminals send --json` narrows it.
Use `orca-bridge doctor --json` to discover actual local compatibility.

The bundled `orca-bridge skill` prints an installable skill. `skill install --claude`
or `--codex` installs only on explicit request and preserves differing existing skills.
Use `schema session`, `schema session-view` and `schema snapshot` for result contracts.
`automations` manages Orca's own scheduled automations; it delegates schedule and
ownership validation to the installed Orca CLI.

## A useful first pass

```sh
orca-bridge recent --limit 10 --json
orca-bridge search 'migration' --agent codex --limit 10 --json
orca-bridge terminals list --json
orca-bridge projects list --json
```

Read a selected session with `sessions show ID --usage --json`. Durable session IDs
look like `local:codex:PROVIDER_ID`; they are not terminal handles. Discover terminal
handles with `terminals list`. Runtime handles expire across runtime/incarnation
changes; reacquire them after errors or reconnects. Do not infer ownership from titles
or directory paths. `open ID` focuses a proven live conversation, or resumes history
only if closure is proven. Unknown ownership fails. `sessions resume ID` explicitly
starts a new terminal for that historical conversation.

## Contracts

- Use `--json` for one JSON result, `--jsonl` for individual list rows. Pipes default
  to JSON; streams always emit JSONL and flush every record. Colors never enter JSON.
- stdout is data; diagnostics are on stderr. Exit 0 is success or a downstream broken
  pipe, exit 1 is a runtime/input failure, and exit 2 is invalid CLI syntax. A timeout
  on a bounded subscription is normal completion; inspect received frames separately.
- Search/recent read Orca's local SQLite index directly and require no pairing or
  running app. Full-text search delegates to Orca's transcript index through its CLI.
- Runtime commands use the local authenticated Unix API. Only native push uses the
  separately paired encrypted WebSocket. Never read or print credential files.
- For literal input, pipe text into `terminals send HANDLE --text - --enter` or
  `sessions create --worktree path:/absolute/project --prompt -`. Never add approval
  or sandbox flags; Orca and the user's agent configuration supply them.
- Native reconnects emit transport records. Reconcile fresh snapshots and notification
  cursors. Rejected/auth-invalid requests fail; they are not endlessly retried.
- Unknown/null model, usage, attention and ownership values mean unknown, not zero/idle.
- Project settings are narrow patches through repo.update. Icon import saves artwork
  in its owning Git repository; apply uploads a validated PNG. Preserve unrelated fields.
- `rpc METHOD`, `rpc --pipe` and `orca -- ARGS` can mutate the runtime. Their effect
  depends on the selected method/command. Discover before using an unfamiliar operation.

Examples:

```sh
printf 'Summarize the current changes' | orca-bridge terminals send HANDLE --text - --enter
orca-bridge stream subscribe session.tabs.subscribeAll --count 1 --timeout 5
orca-bridge watch --topics sessions,status --count 2 --timeout 5
printf '%s\n' '{"id":1,"method":"status.get"}' | orca-bridge rpc --pipe
```

Native pairing and recorder installation are explicit setup actions. A local runtime
credential allows commands as the user. Generate only the requested local credential,
keep the link private, and import it over stdin. Never put it in command arguments,
commits, screenshots, logs or messages. See README.md for enrollment and limitations.
