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

## The 200 ms search regression

The initial bootstrap measured about 192 ms per query across 484 sessions. A fresh
baseline measured 194.5 ms for recent sessions and 216.2 ms for a
fuzzy query, while process help took 4.9 ms.

The cause was `serde_json::from_reader(File)`: serde_json consumed the 484,077-byte
parse cache through an unbuffered File, making tiny reads. Reading the cache once
and parsing the byte slice removed that syscall cost. Whole-process medians dropped
to 9.1 ms for recent sessions and 9.9 ms for the same fuzzy query. Removing repeated
typo-fallback allocations then reduced ranking from about 0.94 ms to 0.32 ms.
The final measured fuzzy-query process median was 9.07 ms across 487 indexed sessions;
history loading was 2.67 ms. These are local warm measurements, not a cross-machine
latency guarantee.

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
