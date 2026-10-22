# Orca Bridge

Rust CLI and reusable library for local Orca search, automation and streams.

- Use Make as the public command interface. mise pins Rust; `make setup` installs it.
- Run `make format`, then `make check` and `make build` before shipping changes.
- Keep JSON/JSONL stdout clean. Diagnostics go to stderr. Broken pipes are successful termination.
- Read Orca-owned databases with read-only connections. Keep writes in our own state directory.
- Never log, export, or commit runtime auth tokens, pairing keys, live session data, or transcripts.
- Keep runtime handles scoped to runtime/incarnation; never infer conversation identity from a path/title alone.
- Make subscriptions reconnectable and cancellation bounded. Preserve unknown/stale state explicitly.
- Use fixtures for tests; live smoke commands are separate from deterministic checks.
- Keep commands and actual compatibility limits documented in README.md and docs/.
- Stage related files explicitly and inspect the staged diff before commits.
