# Compatibility and verification

Current target: macOS and [Orca](https://github.com/stablyai/orca) **1.4.225**. `doctor --json` reports runtime version,
history readability and native pairing status. Pairing status means a private
credential exists; it is not an online health check.

| Surface | Verified |
| --- | --- |
| Local history and metadata search | Read-only live smoke; synthetic schema/cache fixtures |
| Encrypted enrollment | One approved loopback credential; authenticated live status request |
| Native tabs/status/notifications | Real initial frames from all three subscriptions |
| Native follow-up event | Disposable terminal tab created and removed; exact tab ID observed |
| Stream crypto | Shipped TweetNaCl fixture; synthetic encrypted server |
| Project settings/icon RPC | Shipped schema, persisted settings read-back, existing PNG applied |
| Historical resume | Synthetic account/identity preservation; no user's historical session launched |
| Alfred | Script Filter fixtures and packaged workflow; no GUI import claim |
| Raycast | Typecheck and extension build passed; GUI import is separate |
| macOS URL handler | Built, installed and default scheme registration verified; parser fixtures cover valid/invalid URLs |
| UI snapshot | All normalized resources available against live Orca; fixtures reject title/path-only ownership and stale incarnations |
| Session display titles | Reversible, searchable Bridge-owned labels; source index unchanged in fixtures |
| Exact session termination | Disposable PTY stopped and verified; its tab then cleaned up |
| Bundled assistant skill | Frontmatter validator plus non-destructive/idempotent installer fixtures |

## Current limits

- Internal Orca schemas are version-coupled. There is no broad version compatibility
  promise; unsupported schemas fail with diagnostics. Stronger upgrade fencing remains work.
- Some provider identities and statuses remain unknown. Unknown status does not mean idle.
  `open` fails conservatively when it cannot prove ownership. See [identity.md](identity.md).
- Orca's `terminal.focus` currently ignores `expectedIncarnationId`. The bridge checks
  resolution before focusing; the server does not offer an atomic focus guarantee.
- Model/total-token metadata comes from a partial cache and is checked for freshness.
  Uncached active sessions may have no model. Usage reads at most 2 MiB of transcript
  tail; counters can be partial. Claude synthetic messages are excluded.
- `watch` is snapshot polling, not native push. Native consumers must reconcile snapshots
  and notification cursors after reconnect. Polling is still the recorder's source.
- `sessions create --project` creates a worktree and requires `--name`; `--worktree`
  launches in an existing one. Worktree project filtering relies on the runtime's
  `projectId` field and is not verified across remote/project-host arrangements.
- Transcript follow supports local JSONL and truncation. Provider databases and renamed
  transcript files need provider-specific handling.
- Recording defaults to 30-day/100,000-event retention, enforced each minute.
  Check `events status` replay bounds; a stale cursor needs a fresh snapshot.
  Uninstall preserves available history.
- No popup/widget UI is included. Launcher adapters are thin clients of the CLI.

Live credentials, runtime metadata, transcripts and private evidence never belong in
Git. Live smoke commands are explicitly separate from deterministic `make check`.
