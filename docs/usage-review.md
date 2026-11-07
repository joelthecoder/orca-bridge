# Local usage review — 2026-11-03

The review covered the provider transcript files referenced by Orca's local index
across Codex, Claude, Cursor, Gemini and OpenCode. It also inspected available Codex
runtime/archived history outside that index, a private session-audit aggregate and
the lifecycle consumer. Local session counts and benchmark output remain private.
No transcripts, runtime identities, message bodies or credentials are retained here.
Cloud-only chats are unavailable; this is not an audit of every possible chat store.

Conservative review distinguished direct shell Bridge invocations from source-only
mentions, heredoc examples and the current audit where identifiable. Dynamic
subprocess invocations limit completeness. There were no literal Bridge mentions in the indexed
non-Codex transcripts; that does not establish that those agents never used Bridge.

## Findings and changes

| Evidence | Result |
| --- | --- |
| Terminal reads dominated verified invocations. Repeated Python tail-slicing adapters retained drafts and only a small recent suffix. | `terminals read --tail N` trims native tail rows and preserves drafts, opaque cursors and unknown fields, with explicit Bridge trimming metadata. |
| Multiple sessions opened provider JSONLs and unpacked nested message content to retrieve assistant replies. | `sessions messages` supplies bounded recent Codex/Claude text, role filtering, source offsets and explicit source/limit/partial-record metadata. |
| Raw transcript reads could return only the beginning or start at EOF. | `sessions transcript --last N` reads recent nonblank records in order and can follow appends. Zero counts and oversized newline-terminated records are rejected. |
| A live response used decimal string cursors; follow extracted only numeric cursors. Native RPC input requires numbers, so forwarding the string also failed. | Read/follow accepts cursor strings, converts native decimal strings to numeric RPC input, and carries returned cursors into subsequent requests. A supplied unchanged cursor emits nothing. |
| A synthetic fragmented reply succeeded after 12.23 seconds despite a 10-second RPC timeout. Follow also allowed polling sleeps and individual RPCs to exceed its timeout. | RPC fragments/keepalives share one response deadline; follow bounds read time and sleep by its remaining timeout. |
| A fixture reproduced `--path /repo/foo` including `/repo/foobar`. | Local metadata filters now compare directory components. |

The audit also found an attempted `terminals wait --help` discovery probe. This was
not evidence of a failed waiting operation. The documented cursor-based follow
recipe now covers waiting for new output without inventing agent-completion semantics.

Home's lifecycle consumer uses `sessions show` to prove exact provider identity and
checks returned metadata. That verification remains necessary. This review adds no
identity inference from names/paths, background collector or automatic messaging.

## Verification

`make format`, `make check` (50 library and 31 CLI tests), `make build` and
`make smoke` passed. The live smoke includes normalized messages, a raw transcript
suffix, native terminal-tail trimming and cursor-based follow, alongside the
existing history/RPC/watch/replay checks.

Deterministic fixtures cover message projection and omissions, byte/record limits,
partial JSONL, Unicode/boundary cases, tail metadata preservation, native string
cursors, unchanged-cursor waiting, polling and stalled-read deadlines, fragmented
RPC/keepalives and neighboring-directory exclusion. Live read-only smoke checks
remain separate from deterministic validation.
