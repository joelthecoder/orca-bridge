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
orca-bridge sessions messages ID --role assistant --limit 5 --json
orca-bridge terminals read HANDLE --tail 12 --json
```

`sessions messages` projects recent user/assistant text from local Codex and Claude
JSONL into an envelope with `id`, `agent`, `messages`, `sourceTruncated`,
`limitTruncated`, `incompleteFinalRecord` and `bytesRead`. Each message has `role`,
`text`, nullable source `timestamp`/`phase`, and an absolute byte `sourceOffset`.
It omits tool records, thinking, non-text blocks and duplicate Codex event records.
`--role` filters before `--limit`; returned messages retain source order. A source
phase is metadata, not a promise that the agent finished. The default source tail
is 2 MiB; `--max-bytes` accepts 1–16777216. `bytesRead` can include one extra byte for
a record-boundary probe. Check both truncation flags before drawing conclusions
from empty or partial history. Other providers fail explicitly.

`--json-errors` emits a versioned error on stderr while preserving stdout and exit
codes. Use stable codes and safe native recovery IDs; `not-started`, explicit `failed`
and `unknown` outcomes differ. An uncertain action is never automatically retried.
`schema error` describes the envelope. Native helper calls default to 60 seconds
and 8 MiB combined stdout/stderr, configurable with `--command-timeout` and
`--max-output-bytes` (CLI > environment > defaults). Cancellation only stops the
Bridge-owned helper group; a runtime action may already have happened.

Both send aliases support `--wait-submit SECONDS` and `--retry-request REQUEST_ID`
with `--enter`. Observation timeout preserves the queued/accepted native receipt
without resending. Retry requires the original exact payload and incarnation.
Older hosts may lack those guarantees; inspect the receipt and capability evidence.
Use a helper deadline longer than a long submission observation.

Use `sessions transcript ID --last 20` for recent raw JSONL records. `--last` counts
nonblank records, supports `--follow`, and conflicts with `--tail` (start at EOF).
`--count` bounds total emitted records, including the initial suffix. With follow,
an incomplete final record waits for completion and does not consume a suffix slot.

`terminals read --tail N` trims only the native `terminal.tail` array and preserves
other fields. `bridgeTail` reports `limit`, `returned` and whether Bridge shortened
the native tail. Retain Orca's cursor and upstream truncation metadata. Cursors are
opaque strings: copy `terminal.nextCursor` into `terminals follow HANDLE --cursor
CURSOR --count 1 --timeout 30` to wait for changed output without repeated full reads.
An unchanged supplied cursor emits nothing; without a cursor, follow emits an
initial snapshot. Timeout is normal completion, including during a stalled read.
Output arrival alone is not completion or message receipt.

Read a selected session with `sessions show ID --usage --json`. Durable session IDs
look like `local:codex:PROVIDER_ID`; they are not terminal handles. Discover terminal
handles with `terminals list`. Runtime handles expire across runtime/incarnation
changes; reacquire them after errors or reconnects. Do not infer ownership from titles
or directory paths. `open ID` focuses a proven live conversation, or resumes history
only if closure is proven. Unknown ownership fails. `sessions resume ID` explicitly
starts a new terminal for that historical conversation.

## Resource inspection and explicit cleanup

`resources --json` reports schema-versioned app/host and managed PTY process-tree
RSS bytes/CPU percentages. It uses Orca's point-in-time diagnostics endpoint and
sorts terminal rows by memory; `--limit` is 1–2000 and `--terminal HANDLE` selects a
unique verified binding. `schema resources` describes fields. `orcaTotal` and host
numbers remain global when terminal rows are filtered. Null means unknown, not
zero. Unbound rows are visible but do not supply a terminal to stop. A stable runtime
fence and exact worktree/PTY/incarnation metadata establish bindings; titles and
folders do not. PIDs are diagnostic shell roots, not authority for arbitrary kill.

Use `worktrees ps` for attached/live/unverifiable terminal counts and host-scope
coverage. `worktrees usage [--worktree SELECTOR] --limit 20 --timeout 15` measures
local allocated disk estimates and Git dirty/untracked counts with a bounded
budget. Inspect inventory/scan flags and per-measurement availability. RSS and
allocated disk estimates are not guaranteed reclaimable bytes.

Resources support `--repo` or `--worktree` scope before `--sort memory|cpu` and limits.
`selection` records resolved scope; app/host/Orca totals stay global. Worktree usage
supports `--repo` and returns copyable exact `worktreeSelector` values when proven.

After choosing a target, `terminals stop HANDLE --expected-incarnation ID` refuses
a changed terminal, targets the exact current PTY and preserves history. Existing
`sessions stop ID` additionally proves provider ownership. `worktrees remove
SELECTOR` requires a stopped local linked worktree and fresh Git/main/PTY checks.
`--force` permits discarding dirty/untracked Git work only; it does not waive main
or live/unknown-terminal guards. Native removal may delete a merged local branch.
Hooks require `--run-hooks`; waiving a failed hook requires the separate
`--allow-failed-archive-hook`. See [resources.md](resources.md) for race and disk limits.
No automatic reclamation is performed.

`worktrees inventory --include-external [--repo SELECTOR]` reads Git metadata only
for registered local Orca repositories. It preserves archived, detached HEAD,
locked, prunable and missing-path states separately with source/coverage. An entry
omitted by the native page is not necessarily unregistered; only verified native
resolution gives it an exact selector. These facts do not authorize cleanup. No
visibility setting changes, generic scans or pruning are performed.

Preview with `worktrees remove SELECTOR --dry-run`: inspect `allowed`, blockers and
effects; exit 0 means inspection succeeded, not permission to remove. No hooks or
deletion run. Actual removal repeats checks and reports acknowledgement separately
from verified/unknown read-back. Its local operation ID is not a replay token.
Offline schemas include `worktree-usage`, `removal-plan` and `removal-receipt`.

## Cross-project handoffs

For implementation in another project, discover and explicitly select its
worktree, then pipe a self-contained task into `sessions create --worktree SELECTOR
--agent AGENT --prompt - --json`. Keep coordinator research and target implementation
separate. Do not silently fall back to editing another project from the current
session if the handoff fails. This is agent policy, not a filesystem security gate.

Creation requires exactly one of `--worktree` or `--project`; `--project` requires
`--name` for a new checkout. Inspect native `outcome` and `receipt`: launch acceptance,
prompt delivery and task completion differ. After an ambiguous launch reply, inspect
the target and use its original `--retry-launch OPERATION_ID` with the same existing
worktree request, never a fresh duplicate. Native results and errors are preserved.
See [routing.md](routing.md).

## Output and identity contracts

Finite reads support `--fields id,title,agent` for compact top-level JSON selection
(`--jsonl` projects list rows). Missing names become null. Completeness, scope,
warnings/errors and preview blockers/effects remain. Mutations and streams reject
projection before actions start. Use jq for nested queries. `completions
bash|zsh|fish|elvish|powershell` works without Orca or HOME; installation is explicit.

`doctor` separates advertised runtime capabilities from the installed CLI's offline
command/flag registry. Missing evidence stays unknown; CLI/version presence alone
does not prove host support. Discovery makes no mutating probes or memory sweep.
[automation.md](automation.md) documents recovery and capability semantics.

The capability-gated organization commands are `terminals move HANDLE --worktree
SELECTOR` and `sessions names preview --stdin`, `apply PLAN_ID`, `undo CHANGE_ID`.
They delegate whole-tab movement and guarded naming to the native execution host;
they never write Orca's databases. Stock Orca 1.4.242 lacks their advertised
capabilities and is refused before mutation. See
[session organization](session-organization.md) for selection and replay contracts.

- Use `--json` for one JSON result, `--jsonl` for individual list rows. Pipes default
  to JSON; streams always emit JSONL and flush every record. Colors never enter JSON.
- stdout is data; diagnostics are on stderr. Exit 0 is success or a downstream broken
  pipe, exit 1 is a runtime/input failure, and exit 2 is invalid CLI syntax. A timeout
  on a bounded subscription is normal completion; inspect received frames separately.
- Search/recent read Orca's local SQLite index directly and require no pairing or
  running app. Full-text search delegates to Orca's transcript index through its CLI.
- Metadata `--path` filters match path components (the directory and descendants),
  not neighboring directories that merely share a textual prefix.
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
