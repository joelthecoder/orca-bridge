# Automation contracts

The 0.2.0 improvements retain Bridge's role as an explicit local CLI/library.
They add no automatic dispatcher, cleanup daemon or filesystem permission gate.

## Prompt delivery and launch recovery

Both `terminals send` and `sessions send` accept `--wait-submit SECONDS` and
`--retry-request REQUEST_ID`; both require `--enter`. Observation watches an already
accepted prompt and never resends on timeout. Use the returned native receipt to
distinguish accepted input, observed submission and turn start. Task completion is
separate. After an ambiguous reply, repeat the exact payload/flags with the native
request ID; the host binds it to the original terminal incarnation. Older hosts may
support legacy input without replay or observation; do not invent those guarantees.

`sessions create` similarly exposes native `--retry-launch OPERATION_ID` for existing
worktrees. Preserve native outcome/receipt fields and follow [routing.md](routing.md).
There is no unconditional retry loop for any mutation.

## Bounded native helpers

Each delegated Orca CLI helper has a 60-second deadline and an 8 MiB combined
stdout/stderr cap. CLI `--command-timeout` and `--max-output-bytes` take precedence
over their respective `ORCA_BRIDGE_COMMAND_TIMEOUT_SECS` and
`ORCA_BRIDGE_MAX_OUTPUT_BYTES` environment variables, then defaults. Accepted ranges
are 1–3600 seconds and 1–33554432 bytes. Set a deadline longer than the requested
submission observation. These settings do not impose a deadline on the agent task.

Bridge captures pipes with bounded memory and cancellation. It terminates only its
own helper process group, including descendants holding output open. It never kills
Orca's runtime process group to cancel a command. A timed-out or interrupted helper
may already have issued the runtime action; inspect before retrying. Partial helper
output is not emitted as a valid result. Resource and Git inspection budgets remain
separate documented bounds.

## Machine-readable errors

Use `--json-errors` for one versioned JSON object on stderr on failure. Stdout remains
result data; a downstream broken pipe is successful termination. Exit codes remain
0 for success, 1 for runtime/input failure, and 2 for CLI syntax failure. Help/version
still use ordinary successful discovery output.

`schema error` describes the full `{schemaVersion,error}` envelope. Stable error
codes include `invalid_arguments`, `runtime_unavailable`, `rpc_timeout`,
`command_timeout`, `output_limit`, `interrupted`, `invalid_response`,
`native_error`, `preflight_blocked` and generic `request_failed`. Unknown generic
failures remain conservative. `nativeCode`, `operationId`, `retryRequestId` and
`retryLaunchId` are included only when known and allowlisted. Human diagnostics also
include safe recovery IDs. Raw arguments, prompts, credentials, error payloads and
recovery command strings are not copied into envelopes.

`outcome: not-started` means Bridge did not start the action; `failed` requires
explicit outcome evidence; `unknown` means the mutation may have occurred. A native
`ok:false`, lost response, timeout or malformed helper output alone does not prove
that no side effects occurred. Never parse diagnostic prose for retry decisions.
Destructive removal is never automatically replayed.

## Capability discovery

`doctor` includes per-feature states and evidence from the runtime's advertised
capabilities and the installed CLI's offline `agent-context` registry. CLI support
and runtime support are separate observations. When both are required, installed
flags alone do not establish host support. A missing runtime advertisement stays
unknown; an explicit false advertisement is unsupported. Version labels are evidence
metadata, not capability proof. Inherited remote CLI targeting remains explicit and
cannot turn local runtime evidence into a remote-host support claim.

Discovery does not probe launches, sends, stops, hooks, removal or memory sweeps.
Direct RPC features that a runtime does not advertise can remain unknown even when
an explicit normal command succeeds. This is a capability report, not an assertion
that every advertised feature will succeed for every target.

## Scope, compact output and completions

Resources can be narrowed with `--repo SELECTOR` or `--worktree SELECTOR` (mutually
exclusive), plus an exact `--terminal HANDLE`. Scope is applied before `--sort
memory|cpu` and `--limit`. `selection` records the resolved scope. Host/app/Orca totals
remain global; selected terminal counts and truncation remain explicit.

`worktrees usage --repo SELECTOR` forwards the native repository selector. Its
bounded inventory and per-measurement availability are retained; it does not imply
that unmeasured directories are smaller or empty. Copy exact `worktreeSelector`
values into preview/removal, rather than inferring identity from display names.

`--fields id,title,agent` selects top-level field names on finite read results and
implies structured JSON. With arrays, each row is projected. With object envelopes,
the named fields are selected; missing names produce null. Completeness, scope,
version, warnings/errors and preview blockers/effects remain visible automatically.
Nested field expressions and embedded query evaluation are not supported; pipe to
`jq` when needed. Mutations and streams reject projection before any action begins.

`completions bash|zsh|fish|elvish|powershell` generates scripts from the same Clap
parser as command discovery and runs without Orca or HOME. Installation into shell
completion directories is explicit; Bridge does not edit shell startup files.
