# How Orca Bridge works

Orca Bridge has three independent data paths. Searching does not require enrollment.

```text
search / recent ──────── read-only SQLite index + partial parse cache
runtime actions ─────── authenticated local Unix socket
native subscriptions ── paired, NaCl-encrypted loopback WebSocket
```

The Rust library exposes these adapters separately. The CLI adds argument validation,
output formats, literal stdin and launcher-friendly commands. No daemon sits between
a search command and Orca's index.

## Search performance

The search path reads the parse cache once and parses the byte slice. This avoids
the small reads caused by using `serde_json::from_reader(File)` on an unbuffered
file. Fuzzy ranking also avoids repeated allocations during typo fallback.

Query timings depend on the machine, cache and session inventory. Keep live session
counts and workstation measurements out of published examples.

`make profile` reports process startup, whole-process queries, SQLite open/count,
cache read/decode, full history loading and fuzzy ranking separately. It never prints
session contents. Search still validates cache entries against transcript size/mtime;
stale cache data cannot supply a model. No stale search cache or persistent server
was needed for this fix.

## Streams

`watch` compares local snapshots. It emits explicit unavailable/reconnected records,
created/updated/removed diffs and notification catch-up. It optionally records events
in the bridge's private SQLite journal. Replay cursors belong to that journal.

`stream subscribe` uses Orca's native transport. Enrollment verifies an encrypted
`status.get` request before storing a credential. The client advertises the structured
agent capability required by `agentSession.subscribeStatus`. Every subscription gets
a fresh request ID. Consumers receive the original response frame, not an invented
normalized session identity. A normal terminal response ends the subscription;
authentication, protocol and request rejection errors fail rather than retrying.

Native snapshots and tab creation/removal were verified against Orca 1.4.225.
Synthetic servers cover crypto interoperability and reconnecting subscriptions.

## Boundaries

Only Orca writes its databases. The bridge uses read-only SQLite connections and
supported runtime operations for mutations. A session's durable provider ID is not
a live terminal handle. Bridge-owned display titles live in a private label database. They never rewrite
provider transcripts or Orca state. Normalized UI snapshots omit credentials, hook
scripts and previews, and redact Git URL credentials. Runtime handles must be reacquired after restart/incarnation
changes. See [identity.md](identity.md) for ownership evidence and focus limits.
