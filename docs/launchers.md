# Launchers and session links

All adapters invoke the CLI with literal arguments. They do not evaluate search text
as shell code or reconstruct identity from titles.

## Alfred

```sh
make install
make alfred
open 'target/launchers/Orca Bridge.alfredworkflow'
```

Import the workflow in Alfred, then type `orca QUERY`. The Script Filter runs
`search QUERY --limit 30 --alfred`; Return passes the selected durable session ID to
`open`. Alfred's queued query policy drops superseded searches. The default binary
is `~/.local/bin/orca-bridge`; `ORCA_BRIDGE_BIN` overrides it.

The package is built deterministically with Python's plist/ZIP libraries. The CLI's
Script Filter result contract is fixture-tested. Actual Alfred import/interaction
requires Alfred and is separate from packaging validation.

## Raycast

```sh
make install
make raycast
cd integrations/raycast
npm run dev
```

Load the local extension in Raycast's development mode. Its optional binary preference
accepts an absolute CLI path; otherwise it uses `~/.local/bin/orca-bridge`.
Search runs as a direct child process. Changing the query kills the previous process;
results from superseded searches are discarded. Return opens the selected conversation;
the action panel can copy its ID.

`make raycast` installs locked dependencies without lifecycle scripts, typechecks the
source and builds it with Raycast's tooling. Typechecking and the build passed;
the locked dependency audit reported no vulnerabilities. A successful build does not establish
that the extension has been imported or used in Raycast.

## Clickable macOS links

```sh
make url-install
orca-bridge sessions link ID --json
open 'orca-bridge://session/ENCODED_ID'
```

The small AppKit handler installs in `~/Applications/Orca Bridge.app`, registers the
`orca-bridge` URL scheme and bundles the release CLI. It passes the URL literally to
`orca-bridge open`. Rust rejects unexpected hosts, queries, fragments, control
characters and malformed paths. Percent decoding happens exactly once. It retains
the same conservative ownership behavior as opening from the terminal.

Installation refuses to replace an application with a different bundle ID. The
bundle is locally ad-hoc signed. It is a local source installation, not a notarized
public distribution. Run `make url-install` again after updating the CLI. The helper
`Contents/MacOS/OrcaBridge --check-handler` reports and verifies the registered default.
