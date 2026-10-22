<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="Assets/README/logo-dark.svg">
    <img src="Assets/README/logo-light.svg" alt="Orca Bridge" width="600">
  </picture>
</p>

**Find the session. Keep the flow.**

Search your [Orca](https://github.com/stablyai/orca) conversations from a terminal or launcher. Jump back into the right
session, send an agent a prompt, inspect usage, and turn workspace activity into Unix
pipelines. A Rust CLI and reusable library, built for people **and** automation.

```sh
orca-bridge search 'that migration we discussed'
orca-bridge recent --limit 10
orca-bridge open local:codex:YOUR_SESSION_ID
```

Metadata search reads Orca's local SQLite index directly. **No running app, pairing,
server, or background collector is needed to search.** Warm whole-process queries
measured around **9 ms across 487 sessions** on the development Mac; measure your own
machine with `make profile`. [How it works and performance](docs/architecture.md).

| You want to… | Use Orca Bridge to… |
| --- | --- |
| Find a conversation you lost | Fuzzy-search titles, projects, branches, agents and models; search transcript content with `--full-text` |
| Pick up where you left off | Open a proven live conversation, or explicitly resume a historical session |
| See what needs you | Inspect live terminals, agent status, recent sessions and explicit attention |
| Automate an agent | Create a session, send literal stdin, read terminal output and follow its transcript |
| Build a launcher or dashboard | Consume JSON/JSONL, native push subscriptions, or a replayable local event journal |
| Schedule recurring work | Create, inspect, edit and run Orca’s built-in automations |
| Build your own UI | Export versioned snapshots and generated JSON Schemas |
| Manage your projects | Add repositories, create worktrees, patch project settings and apply repository-owned icons |

macOS first. Verified with **Orca 1.4.225** ([releases](https://github.com/stablyai/orca/releases)). Internal APIs can change; `doctor` reports
what is actually available. [Compatibility and limits](docs/compatibility.md).

## Get started

From this checkout, with [mise](https://mise.jdx.dev/) installed:

```sh
make setup                 # Install the pinned Rust toolchain and fetch dependencies
make check                 # Formatting, Clippy and deterministic behavioral tests
make install               # Build and install ~/.local/bin/orca-bridge
orca-bridge doctor --json
```

Put `~/.local/bin` on your shell's `PATH` if needed. `make` shows the available project
commands. Use `INSTALL_DIR=/your/bin` to choose another install directory.

```sh
orca-bridge --help                  # Human help; colors in a terminal
orca-bridge search --help           # Options for a specific command
orca-bridge help --json             # Complete machine-readable command tree
orca-bridge help terminals send --json
orca-bridge agent                   # Portable guide for agents and tool authors
orca-bridge skill                   # Print the bundled assistant skill
orca-bridge skill install --codex    # Or --claude; explicit, preserves existing skills
```

Help and the agent guide work without Orca, credentials, or even a configured data
folder. Human output uses cyan commands and titles, amber accents and muted details.
Use `--human` to keep that layout in a pipe, `--color always|never|auto` to control
styling, or `NO_COLOR=1` to disable automatic colors. JSON is always unstyled.

## Find the conversation, then open it

```sh
orca-bridge search 'migration'
orca-bridge search 'cafe 東京' --agent claude --limit 20
orca-bridge search 'release' --path /absolute/project --since 2026-10-01T00:00:00Z
orca-bridge search 'retry budget' --full-text --json
orca-bridge recent --limit 5 --jsonl
```

Metadata search tolerates fuzzy abbreviations and small typos. Full-text search uses
Orca's conversation index through its installed CLI. Search results include a durable
`id`; copy it into the next command:

```sh
orca-bridge sessions show ID --usage --json
orca-bridge open ID
orca-bridge sessions resume ID
orca-bridge sessions link ID
```

`open` focuses a live conversation only when its provider identity is proven. If it
can prove the conversation is closed, it resumes history. If ownership is uncertain,
it says so. `resume` explicitly asks for a new terminal. Titles and matching folders
are never treated as identity. [Identity guarantees](docs/identity.md).

## Talk to your agents

List the live terminals to get a runtime-issued **terminal handle**. A session ID and
a terminal handle identify different things; use the kind each command asks for.

```sh
orca-bridge terminals list --json
orca-bridge terminals read HANDLE --json
orca-bridge terminals follow HANDLE --timeout 30
printf '%s' 'Summarize the changes and run the project checks.' \
  | orca-bridge terminals send HANDLE --text - --enter
```

Start an agent in an existing worktree, using Orca's configured accounts and launch
preferences:

```sh
printf '%s' 'Review this project and explain the next useful change.' \
  | orca-bridge sessions create --worktree path:/absolute/project --agent codex --prompt -
orca-bridge sessions transcript ID --follow --tail --timeout 30
orca-bridge sessions attention --json
```

Send a file or a diff as context with `cat context.md | orca-bridge sessions send
HANDLE --text - --enter` or `git diff | orca-bridge sessions send HANDLE --text - --enter`.
Omit `--enter` to leave the input ready for review. These commands target terminal
sessions; native Chat UI messages use a separate protocol.

Rename or clean up selected sessions:

```sh
orca-bridge sessions rename ID 'Release review'  # Persistent Bridge display title
orca-bridge sessions rename ID ''                # Reset to the indexed title
orca-bridge sessions stop ID                     # Exact proven PTY; preserves history
orca-bridge terminals stop HANDLE
orca-bridge terminals close HANDLE --tab
```

Bridge titles are searchable and preserve `originalTitle`. Live tab rename is also
available as `terminals rename HANDLE TITLE`; its result distinguishes acknowledgement
from read-back verification. Orca 1.4.225 can acknowledge a rename while its title
snapshots remain stale.

Prompt text is passed literally. Agent approval and sandbox settings come from Orca
and the user's configuration. Optional `--model`, `--effort` and `--focus` select the
launch behavior. Historical usage is bounded and provider-dependent; missing or partial
counters stay explicitly unknown.

## Compose it with other tools

Pipes default to JSON. `--json` produces one result; `--jsonl` produces one list entry
per line. Streams always produce JSONL and flush each record. Diagnostics go to stderr;
a downstream broken pipe is a successful exit.

```sh
# Search a supplied session list without touching Orca's index.
orca-bridge recent --limit 100 --jsonl \
  | orca-bridge search codex --stdin --jsonl

# Filter arbitrary JSONL by a selected field.
printf '%s\n' '{"title":"Rust bridge"}' '{"title":"Other task"}' \
  | orca-bridge filter rust --field /title

# Make multiple local API calls, with one correlated result per request.
printf '%s\n' '{"id":1,"method":"status.get"}' \
  '{"id":2,"method":"project.list"}' | orca-bridge rpc --pipe
```

Agents can discover arguments, defaults, allowed values and command effects from
`help --json`. The schema is generated from the same Clap parser as the CLI.
[Agent guide and output contracts](docs/agent-guide.md).

### Pick a session with fzf

With `jq` and `fzf` installed, load recent history once and filter as you type:

```sh
orca-bridge recent --limit 2000 --json \
  | jq -r '.[] | [.id, .title, .agent, .cwd] | @tsv' \
  | fzf --delimiter '\t' --with-nth=2.. \
      --preview 'orca-bridge sessions show {1} --json' \
  | cut -f1
```

The picker displays title, agent and folder; stdout contains the selected durable
session ID. Escape cancels. Filtering happens inside `fzf`, with no process launch
per keystroke. [Open on Enter, or reload Bridge search as you type](docs/fzf.md).

## Build a UI on top

```sh
orca-bridge snapshot --limit 100 --json
orca-bridge schema session
orca-bridge schema session-view
orca-bridge schema snapshot
```

A snapshot combines history, projects, repositories, worktrees, terminals and tabs.
Session rows expose **project/repository names and IDs, folders, Git remotes, model,
title, timestamps and usage metadata**, with explicit unknowns and truncation flags.
Proven live bindings carry terminal handles, incarnation IDs and identity evidence.
A folder match links project metadata; it never proves conversation ownership.

Every resource reports availability, so a UI can still show history during a runtime
outage. Schema and command discovery work without the app. Add `--icons` when a UI
needs project artwork. [Data schemas, relationships and UI actions](docs/ui-data.md).

## Orca's built-in automations

```sh
orca-bridge automations list --json
orca-bridge automations show ID --json
printf '%s' 'Review changes and report anything that needs attention.' \
  | orca-bridge automations create --name 'Daily review' --trigger daily \
      --time 09:00 --timezone America/Edmonton --repo name:project \
      --agent codex --prompt - --disabled
orca-bridge automations edit ID --enabled
orca-bridge automations run ID
orca-bridge automations runs --id ID --json
```

These manage **Orca automations**, using Orca's own scheduler and ownership checks.
Triggers accept presets, five-field cron or RRULE. Use `--worktree` for an existing
workspace or `--repo` for a new worktree per run. `--disabled` saves a draft without
scheduled execution. Edit patches named fields; `remove ID` also removes its run
history. Extra version-specific controls remain available through `orca -- automations …`.

## Follow activity as it happens

Two complementary ways to watch:

| Mode | Best for | Requires |
| --- | --- | --- |
| `watch` | Unified session/terminal/project changes and optional journal recording | Local Unix API and read-only SQLite snapshots |
| `stream subscribe` | Orca's native encrypted push events | One separately enrolled runtime credential |

```sh
orca-bridge watch --topics sessions,status,notifications --timeout 30
orca-bridge watch --topics sessions,terminals --record
orca-bridge events list --since-id 0 --limit 100 --jsonl
orca-bridge events follow --since-id 100
```

`watch` polls at 1000 ms by default; `--interval-ms` sets its cadence, minimum 100.
Recording goes to Orca Bridge's private state directory. Replay IDs are persistent
local cursors. Notification catch-up deduplicates Orca's epoch and sequence. Recording prunes events
older than 30 days and beyond 100,000 entries at startup and every minute; configure
`--retention-days` and `--retention-count`. `events status` reports replay bounds;
`events prune` applies explicit retention. Monotonic event IDs and notification cursors
survive pruning.

For native push, create **one local-only access link** in Orca:
Settings → Remote Orca Servers → Share this host → This computer only → Generate
Access Link. Copy **Pair another Orca client**, then import privately:

```sh
pbpaste | orca-bridge stream pair --stdin
orca-bridge stream status --json
orca-bridge stream subscribe session.tabs.subscribeAll --count 1 --timeout 10
orca-bridge stream subscribe agentSession.subscribeStatus
orca-bridge stream subscribe notifications.subscribe --params '{"includeDesktopSuppressed":true}'
```

The credential allows commands as you. Keep the link private. The bridge verifies it
before saving it with owner-only permissions. Native tab, agent-status and notification
subscriptions have been verified against real Orca, including a disposable tab's
creation and removal. Transport loss can reconnect; request rejection terminates.
Consumers must reconcile snapshots and cursors after reconnect.

An optional recorder can run as a per-user LaunchAgent:

```sh
orca-bridge service install        # Use the installed binary in a stable location
orca-bridge service status
orca-bridge service uninstall     # Preserves recorded history
```

No recorder is installed by building the project.

## Projects, settings and artwork

```sh
orca-bridge projects list --json
orca-bridge projects add /absolute/project
orca-bridge worktrees list --json
orca-bridge worktrees create --repo name:project --name experiment --agent codex
orca-bridge projects settings show name:project --json
orca-bridge projects settings set name:project \
  --updates '{"displayName":"My Project","badgeColor":"#54ddbb"}'
```

Settings are narrow patches through the verified `repo.update` API, followed by a
read-back check. Only supplied fields change. Supported settings include the display
name, badge color, worktree defaults and visibility, issue source and fork sync mode.

Keep reusable artwork with the repository that owns it:

```sh
orca-bridge projects icon import path:/absolute/project --file /path/to/square.png
orca-bridge projects icon apply path:/absolute/project \
  --file /absolute/project/Assets/Orca/ProjectIcon/icon.png
```

Import stores `master.png`, `icon.png` and reuse instructions without overwriting
existing artwork. Apply fully decodes the PNG before uploading it: static, square,
1–1024 pixels, maximum 256 KiB; 256 × 256 is recommended. Other Orca settings stay
untouched. [Project settings and artwork](docs/projects.md).

## Launch from anywhere

- **Alfred:** `make alfred`, then import `target/launchers/Orca Bridge.alfredworkflow`.
  Type `orca` followed by a query; Return opens the selected conversation.
- **Raycast:** `make raycast` builds and typechecks the local extension. Load it as a
  development extension; it searches the same CLI and cancels superseded queries.
- **Session links:** `make url-install` installs a small macOS handler in
  `~/Applications/Orca Bridge.app`. Links from `sessions link ID` can then open through
  macOS. The handler bundles the CLI; rerun the target after updating it.

[Launcher setup and verification limits](docs/launchers.md).

## Under the hood

Search uses read-only connections to Orca-owned databases and its partial parse cache.
Runtime operations use its authenticated local Unix API. Native push uses an encrypted
NaCl WebSocket. All writes owned by the bridge go to its own state directory, installed
integration files, or explicitly selected repository artwork.

| Location | Default |
| --- | --- |
| Orca data | `~/Library/Application Support/orca` |
| Bridge journal and credential | `~/Library/Application Support/orca-bridge` |
| CLI | `~/.local/bin/orca-bridge` |

Override data/state with `--data-dir`, `--state-dir`, `ORCA_USER_DATA_PATH` or
`ORCA_BRIDGE_STATE_DIR`. `--profile-db` selects a profile explicitly; default discovery
follows Orca's active profile index. `ORCA_BRIDGE_ORCA_CLI` selects the installed Orca
CLI. `orca-bridge orca -- ARGS` exposes commands the bridge has not wrapped yet;
`rpc METHOD --params JSON` calls the private local API. Both can mutate Orca.

## Develop and verify

```sh
make format
make check          # No Orca app, credentials or live data required
make build          # Optimized, locked release build
make smoke          # Read-only checks against running Orca
make smoke-native   # Native push; creates and closes one disposable terminal
make profile        # Read-only phase and whole-process latency measurements
```

Rust is pinned by mise and `rust-toolchain.toml`. Fixtures are synthetic; live state,
transcripts and credentials stay out of Git. Contributions should preserve clean
JSON output, bounded cancellation, exact runtime/session identity and explicit unknowns.

[Architecture](docs/architecture.md) · [Compatibility](docs/compatibility.md) ·
[Agent guide](docs/agent-guide.md) · [UI data](docs/ui-data.md)

## License

[MIT](LICENSE). Orca Bridge is an independent project and is not affiliated with or
endorsed by [Orca](https://github.com/stablyai/orca) or stablyai. Orca's name and mark
belong to their owners.
