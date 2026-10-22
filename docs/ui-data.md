# Data for launchers, dashboards and other UIs

Use the same Rust core and CLI as the terminal interface:

```sh
orca-bridge snapshot --limit 100 --json
orca-bridge schema session
orca-bridge schema session-view
orca-bridge schema snapshot
orca-bridge help --json
```

`schema` prints JSON Schema generated from the Rust/Serde models. It works offline.
`help --json` describes commands, global options, defaults, allowed values, conflicts,
examples and effects. It is generated from Clap, so adding a real argument updates
discovery. These are different contracts: data schemas describe results; help describes
how to invoke operations.

## Snapshot contract

The snapshot's `schemaVersion` is 1. Its resources are `runtime`, `sessions`, `projects`,
`repositories`, `worktrees`, `terminals` and `tabs`. Each has the same envelope:

```json
{"available":true,"data":[],"error":null}
```

Runtime data is an object instead of a list. An unavailable resource has `data:null`
and an error. Other resources can still succeed. History survives runtime outages.
`historyTruncated` and `worktreesTruncated` explicitly describe bounded inventories.
The default history limit is 100, maximum 2000. `--icons` includes project artwork;
without it, inline PNGs stay out of the snapshot.

The bridge captures runtime IDs before and after live inventory. If it cannot fence
one runtime, it invalidates the live resources instead of combining stale handles
with new ones. This is a consistent-runtime check, not an atomic graph snapshot;
individual live resources can still change while collected. Native subscription
snapshots carry publication epochs and versions for reconciliation.

## Session fields

`recent`, `search` and `sessions show` return a history `Session`. Snapshot session
rows extend it as `SessionView`:

| Field | Meaning |
| --- | --- |
| `id` | Durable Bridge ID: `local:<agent>:<provider-session-id>`, components percent encoded |
| `sessionId` | Original provider conversation ID |
| `host`, `agent` | Execution host and provider/agent ID |
| `title` | Bridge display title, or original indexed title |
| `originalTitle` | Present when a Bridge title overrides the index |
| `cwd`, `branch` | Indexed working directory and branch |
| `model` | Fresh partial-cache model; null when unknown |
| `createdAt`, `updatedAt` | Provider/index timestamps, nullable |
| `messageCount`, `totalTokens` | Indexed count and partial-cache token total |
| `filePath`, `codexHome` | Local transcript/account paths, nullable; no credential values |
| `usage` | Optional provider-specific bounded usage result |
| `score` | Optional fuzzy-search ranking score |
| `workspaceMatches` | IDs of local worktrees whose path exactly matches indexed cwd |
| `workspaceContexts` | Folder, repository/project IDs and names, redacted Git remote, association evidence |
| `liveBindings` | Proven terminal handle, incarnation, tab, runtime ID and identity evidence |
| `liveState` | `live` only with proof, otherwise `unknown` |

A session does not become closed/idle because it lacks a live binding. The fast UI
projection uses exact Orca provider ID + agent + terminal incarnation evidence; it
skips expensive per-process transcript inspection. Targeted `open`/`stop` resolution
can additionally use fenced process evidence. Matching titles, project names or folders
never proves live conversation ownership.

Workspace contexts only associate location metadata. Multiple matches are retained;
missing metadata stays null. Follow `projectId`/`repositoryId` into the separate
inventories for colors, artwork, names and paths. Git URL userinfo, query and fragments
are removed. Runtime auth metadata, pairing tokens, hook scripts and terminal previews
are not included in the normalized snapshot.

Tab custom titles and history display titles are distinct. Worktree `identity` is an
Orca-owned object, not a string. Tab `status`/`agentStatus`, icons and workspace status
are nullable, version-dependent Orca values; inspect them before making provider-specific
UI assumptions. Use the generated schema for exact types.

## UI actions

```sh
orca-bridge sessions rename ID 'Release review'
orca-bridge sessions rename ID ''                   # Reset the Bridge display title
orca-bridge open ID
orca-bridge sessions resume ID
orca-bridge sessions stop ID
orca-bridge terminals rename HANDLE 'Release review'
orca-bridge terminals stop HANDLE
orca-bridge terminals close HANDLE --tab
```

Bridge renaming writes only a private local label, appears in subsequent search and
snapshot output, preserves `originalTitle`, and leaves Orca/provider history unchanged.
Live tab rename uses Orca's API; the result distinguishes `acknowledged` from `verified`.
In 1.4.225, a disposable tab's rename was acknowledged but subsequent runtime titles
remained stale. Do not infer confirmed UI persistence from that acknowledgement.

`stop` resolves the target, uses `terminal.stopExact` with its exact PTY ID and
`targetOnly:true`, preserves history, and requires Orca's post-stop verification.
Session-level stop requires exactly one proven terminal. Unknown/multiple ownership
fails. `close` uses Orca's explicit pane/tab close operation; it is separate from
stopping an exact process. There is no title/path-based bulk cleanup.

## Updating a UI

Start with a snapshot, then use native `stream subscribe session.tabs.subscribeAll`
and `agentSession.subscribeStatus`, or polling `watch`, to trigger refreshes. Reconcile
fresh state after reconnect. The journal offers durable replay IDs; `events status`
reports the oldest and newest available IDs. Retention bounds apply, so check your
cursor against those bounds and refresh a snapshot after a gap.

Still useful follow-ups: native Chat UI message/turn controls and title editing,
provider-specific conversation history adapters, paginated history snapshots, richer
model/catalog discovery, and stronger upgrade compatibility tests. No popup/widget UI
is included; this is the data and control foundation for building one.
