# Conversation identity and actions

Verified against Orca 1.4.225 on macOS, 2026-10-21. Packaged JavaScript is readable; native decompilation was unnecessary. Prior REA artifact evidence: `ev_37cc267218d20bc22002f7d56689a79783174a081428aedbcdbabfa37f3016c2`; selected JavaScript evidence: `ev_4988bd172e799eb32cde607daa10dd419c7ac193105ef59c789931fc7d61d0fa`.

## History and live processes

History identity consists of execution host, agent, and provider conversation ID. Live terminal identity consists of runtime ID, handle, and PTY incarnation. Working directory, branch, title, and activity time do not connect them.

`identity::resolve` reads fresh terminal liveness and `session.tabs.listAll`. When a current terminal incarnation includes `agentStatus.providerSession`, its provider ID and optional transcript path supply the association. This field exists in the shipped implementation but was absent in the sampled desktop terminal snapshots.

The verified fallback is `terminal.inspectProcess` with `scanChildProcesses: true` and expected incarnation. Its `foregroundProcessEvidence` includes a live verdict, PTY incarnation, foreground PID/start time, shell start time, TTY, and foreground process group. A sampled Codex process had no conversation ID in its command arguments, but `lsof -b -nP -p PID -Fn` exposed its exact open rollout JSONL file.

The implementation validates process name, compares start time using `ps`, reads its open file names, matches canonical transcript paths, and rechecks Orca's complete process fence. Process arguments, environments, transcript text, and runtime auth tokens are never included in the identity result. Unresolved terminal identities remain explicit.

A Codex process can own several open rollout files: the main conversation and its subagents. This is verified by bounded `session_meta` reads: the main source is `cli`, and children name the same `parent_thread_id`. An exact subagent transcript match reveals its owning terminal; it does not claim to select a child conversation within the Codex TUI. Explicit `terminal_gone` responses exclude retained orphan listing rows.

This fallback recognizes Codex rollout files and Claude project JSONL files. Claude can close transcripts between writes. Foreground subprocesses, missing permissions, unavailable `lsof`, remote hosts, and unsupported agents remain unknown. There is no directory/title guess.

## Opening and resuming

Open focuses one proven live terminal. Multiple proven terminals require explicit selection. No match plus any unknown candidate identity refuses automatic creation of another writer. Explicit `sessions resume` requests historical resume despite that uncertainty; Orca preparation still rejects durable ownership conflicts known to Orca.

Resume calls `aiVault.prepareSessionResume` to respect ownership and Codex account-home preparation. The history `codex_home` column preserves account routing. Preparation may substitute a home or request the real home; real-home resumes remove inherited `CODEX_HOME` and `ORCA_CODEX_HOME`. Creation receives a mutation UUID, `resumeProviderSession`, and a known provider resume invocation. Stored `resume_command` text is never executed. Arguments and directory are quoted, and `/bin/sh -c` defines the inner command grammar on this POSIX adapter. The original directory must exist and resolve to an Orca workspace. Unsupported providers fail explicitly.

The focus schema accepts `expectedIncarnationId`, but the inspected handler does not forward it. Resolution validates incarnation before focus; a process can still exit in that short interval. This is a compatibility limit, not an atomic focus guarantee.

## Links and automation

`orca-bridge://session/<encoded-durable-id>` is the companion protocol. The `url` crate constructs the path. Parsing validates the route and decodes exactly once; durable IDs can themselves contain percent escapes. A launcher calls the CLI URL action, which uses the conservative open behavior. Generating links does not register an OS URL handler; optional macOS helper packaging owns registration.

CLI passthrough passes argument vectors directly through `std::process::Command`, sets the configured data path, requests JSON, and retains Orca mutation error/recovery information. This covers project/worktree creation, agent launch, terminal creation/input, and future commands without duplicating Orca's command model.

Read/follow retains cursor metadata and must report incarnation resets. Prompt input uses Orca's documented command/retry receipt rather than blindly retrying a transport write.

## Evidence and source map

Read-only runtime probes used bundled `RuntimeClient` for `terminal.list`, `session.tabs.listAll`, and `terminal.inspectProcess`. The token remained inside the bundled client. Filtered process evidence established the field schema, `ps` matched start time, and the open-file list contained one Codex rollout. Existing terminals received no input. A separate isolated shell fixture verified terminal create, marker input, bounded read, focus, and close; cleanup completed. The Rust library resolver also found exactly one proven live owner for a known Codex transcript.

Relevant shipped sources:

- `out/shared/rpc-contract/terminal-unary-params.js`: process fences, resume metadata, creation/focus schema.
- `out/shared/agent-session-resume.js`: provider identity normalization.
- `out/shared/agent-resume-argv.js`: resume argument vectors.
- `out/shared/rpc-contract/ai-vault-params.js`: preparation request schema.
- `out/shared/ai-vault-resume-command.js`: account-home conventions.
- `out/main/index.js`: runtime handlers and snapshot enrichment.
