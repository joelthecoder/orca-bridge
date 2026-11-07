# Resources and deliberate cleanup

Bridge exposes measurements and explicit actions. It does not choose cleanup
targets or perform automatic reclamation.

## Memory and CPU

`orca-bridge resources --limit 20 --json` calls Orca's supported `diagnostics.memory`
endpoint. It returns app main/renderer/other totals, host available-memory data,
worktree aggregates and memory-ranked managed-terminal rows. `schema resources`
describes the versioned projection; `--terminal HANDLE` selects a verified row.

The native diagnostics session ID is a PTY ID, and its PID is the shell root. The
measurement includes descendants, rather than only the foreground agent process.
The verified native metric is resident memory in bytes (`processMemoryMetric: rss`)
and OS CPU percentage, which can exceed 100 for a process tree. App/PTY RSS sums
include shared mappings and are not a prediction of physical memory freed by a stop.
Host available memory is a different measurement; its source remains explicit.
Historical `sessions usage` continues to mean provider token usage.

A `status.get` runtime fence surrounds collection and terminal metadata. Handles
are attached only for a unique current local terminal with the exact worktree/PTY,
connected state and incarnation. Missing proof yields a null handle and unavailable
binding, preserving measured usage. Unknown values remain null. No title, folder
or PID-only inference creates a cleanup target. Native histories, prompts and raw
unknown fields are omitted. Snapshot failures remain visible in errors/warnings.

`terminalCount` counts selected rows before `--limit`; `terminalsTruncated` reports
the output limit. `orcaTotal` and host metrics stay global when filtering terminals.
Collect on demand: this host process sweep is intentionally not a cheap heartbeat.

## Worktree inspection

`worktrees ps --json` projects native worktree liveness, including attached/live/
unverifiable PTY counts and host coverage, without previews or agent prompts.
`worktrees usage --worktree SELECTOR --json` inspects one selected tree;
omitting the selector measures a bounded inventory (`--limit 20`, `--timeout 15`).

Disk measurement uses local allocated blocks, including ignored/build files, with
bounded execution and output. Git status is read-only and reports counts rather
than filenames. Remote or unknown execution hosts, missing paths, scan deadlines
and failed measurements remain unavailable rather than zero. Check inventory
truncation, scan-limit and per-row availability before deciding. APFS cloning,
compression, shared extents and shared Git storage mean allocated size is an
estimate rather than a guarantee of reclaimed disk space.

## Explicit actions

`terminals stop HANDLE --expected-incarnation ID` verifies the current incarnation,
targets exact PTY IDs through Orca and preserves conversation history. Existing
`sessions stop ID` resolves provider identity first. Refresh resource measurements
after stopping; app caches, detached children and shared memory can limit reclamation.
Diagnostic PIDs are not a generic process-kill interface.

`worktrees remove SELECTOR` performs fresh local main/base, Git and PTY checks and
delegates deletion through Orca's exact worktree identity. It refuses implicit
`active`/`current` targets, main/base repositories, live or unverifiable terminals,
and dirty/untracked work by default. Stop selected terminals separately first.
`--force` permits dirty/untracked removal only; it does not override the other guards.

Native removal also attempts deletion of the checked-out local branch, retaining
branches known to predate the worktree or not proven merged. Archive hooks skip
by default. `--run-hooks` executes them; failure blocks deletion unless the caller
separately supplies `--allow-failed-archive-hook`. Bridge does not retry destructive
actions automatically. Native errors and results are preserved.

Identity selection protects against deleting a replacement at a reused path, but
the native API does not atomically freeze Git state or new terminal activity across
preflight and deletion. Coordinate ownership before removal; this is an explicit
action interface, not a concurrency lock or an automatic safe-to-delete verdict.

## Scope, preview and receipts

Resources support `--repo SELECTOR` or `--worktree SELECTOR` (mutually exclusive),
applied before `--sort memory|cpu` and `--limit`. `selection` records the resolved
scope; app/host/Orca totals remain global. Worktree usage supports `--repo`, with the
same bounded inventory rather than a claim of globally largest directories. Exact
`worktreeSelector` values are returned when native instance identity is verified.

Preview with `worktrees remove SELECTOR --dry-run --json`. It shares execution's
read-only preflight and returns `allowed`, structured `blockers`, exact target,
checks and planned effects. It never runs hooks or deletion. A blocked preview is
a successful inspection (exit 0 with `allowed:false`); attempted blocked removal
fails as `preflight_blocked`/`not-started`. Execution always repeats fresh checks.

The removal receipt wraps `nativeResult`, exact target, effects and read-back.
`acknowledged` records native success; `verified` additionally requires directory
absence, complete inventory absence of that instance and a stable runtime. Otherwise
the outcome remains `unknown`. Its Bridge `operationId` is a correlation ID, not a
native replay token. Never automatically repeat destructive removal after uncertainty.

Offline contracts are `schema worktree-usage`, `schema removal-plan` and
`schema removal-receipt`. [automation.md](automation.md) defines typed errors and
native helper bounds. Compact projections retain preview blockers/effects.

## Hidden and detached inventory

`worktrees inventory --include-external [--repo SELECTOR] --limit 200 --timeout 15`
adds bounded read-only Git worktree metadata for verified local repositories already
registered in Orca. It is not a filesystem-wide scan or a generic Git pruner. Without
`--include-external`, Git annotates returned native rows and omitted entries are
counted as `excludedExternalCount` rather than added to the page.

Each row records `source` and `listedByOrca`, nullable native `isArchived`, actual
Git `detachedHead`, `locked`, `prunable`, and filesystem `pathExists`. These are
different states. A detached HEAD does not imply abandoned work; a missing path
can leave administrative metadata with little reclaimable disk space. Visibility
settings are reported when known and never changed.

`git-discovered` means absent from the current native inventory page, not necessarily
unregistered. Orca can resolve some such entries by path even when its normal list
omits them. `nativeResolution` reports that result; only a verified exact instance
gets a `worktreeSelector`. The same removal preview/guards still apply afterward.

Check `inventoryTruncated`, `scanLimited`, host scope, repository inspection counts
and `unavailableRepos`. Unknown/remote hosts are never scanned locally. There is no
safe-to-prune score or automatic deletion, and the command never runs archive hooks,
`git worktree prune` or native removal. `schema worktree-inventory` is available offline.
