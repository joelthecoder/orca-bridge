# Changelog

## 0.2.0 — 2026-11-07

### Added

- App/host and per-terminal process-tree memory/CPU snapshots, repository/worktree
  scopes, ranking, coverage flags and generated resource schemas.
- Bounded local worktree disk/Git inspection, prompt-free process summaries, exact
  selectors, guarded removal previews and conservative verified/unknown receipts.
- Opt-in Git-inclusive worktree inventory limited to registered repositories, with
  distinct archived/detached/locked/prunable/missing states and coverage evidence.
- Prompt submission observation and original-request replay on both send aliases.
- Bounded native CLI helpers with explicit deadline/output settings, cancellation,
  typed JSON errors on stderr and allowlisted recovery IDs.
- Evidence-based runtime/installed-CLI capability reporting in `doctor`.
- Compact top-level JSON field selection and offline shell completion generation.
- Bounded normalized Codex/Claude messages and recent raw JSONL suffix reads.

### Changed

- Session creation requires exactly one explicit existing worktree or new-project
  target. Empty and implicit `active`/`current` targets are refused. Existing-worktree
  launches expose native operation replay through `--retry-launch`.
- Native helper calls now default to a 60-second deadline and 8 MiB combined output
  cap. Configure explicit limits for long observations or unusually large results.
- Native error codes/recovery IDs are exposed without dumping raw error payloads.
  `--json-errors` preserves existing exit codes and does not mix errors into stdout.
- Terminal cursors round-trip native decimal strings to numeric RPC input. Follow
  waits only for changed output when given a prior cursor and respects its timeout.
- Metadata path filters compare directory components rather than text prefixes.
- Transcript record bounds cover newline-terminated records; zero counts fail.

### Migration

- Replace caller-folder-default launches with `sessions create --worktree
  path:/absolute/project ...`. Use `--project ID --name NAME` only for a new checkout.
- Prefer `--json-errors` for automation; inspect outcome certainty and native recovery
  IDs. Never treat a timeout as authorization for a fresh duplicate action.
- Removal is explicit and local-only. Preview first; main/base, live/unknown and dirty
  guards remain in effect. `--force` permits dirty work only, and native removal may
  delete a provably merged local branch. No automatic cleanup is installed.
