# Project settings and artwork

Project settings address the repository backing an Orca project. Select it with
`name:project`, `path:/absolute/repository`, or a runtime repository selector.
`projects list` provides project inventory; `projects settings show SELECTOR --json`
returns the backing repository settings.

```sh
orca-bridge projects settings set name:project --updates '{"displayName":"New name"}'
printf '%s' '{"externalWorktreeVisibility":null}' \
  | orca-bridge projects settings set name:project --stdin
```

The bridge resolves the selector to a durable repository ID before writing, patches
only supplied fields through `repo.update`, and reads them back to verify persistence.
It does not read-modify-write unrelated settings. A request with invalid or unknown
fields fails locally instead of relying on Orca's sometimes-silent normalization.

| Field | Values |
| --- | --- |
| `displayName` | String |
| `badgeColor` | Lowercase `#rrggbb` |
| `worktreeBaseRef` | String |
| `worktreeBasePath` | String |
| `externalWorktreeVisibility` | `"show"`, `"hide"`, `null` to inherit |
| `agentWorktreeVisibility` | `"show"`, `"hide"`, `null` to inherit |
| `issueSourcePreference` | `"auto"`, `"upstream"`, `"origin"` |
| `forkSyncMode` | `"ask"`, `"safe-auto"`, `"off"` |
| `repoIcon` | Validated PNG upload object, or `null` to reset; prefer the icon command |

Other settings remain available through raw RPC, with Orca's version-specific schema.
`repo set` in Orca's public CLI only changes visibility. The private `repo.update`
runtime method supports the additional settings, including `repoIcon`; this was verified
from the shipped schema and actual runtime read-back, not inferred from `repo set`.

## Import and apply

```sh
orca-bridge projects icon import path:/absolute/project --file /path/to/square.png
orca-bridge projects icon apply path:/absolute/project \
  --file /absolute/project/Assets/Orca/ProjectIcon/icon.png
```

Import creates `Assets/Orca/ProjectIcon/master.png`, `icon.png` and `README.md` in a
local Git repository. The PNG is copied exactly; no automatic resizing changes the
source. Existing different art is never overwritten. If you already have a richer
master, keep it and apply your existing upload PNG directly.

Apply checks the entire decoded PNG, square dimensions, static format, decoded-size
limit and Orca's 256 KiB upload limit. It uploads one icon field and verifies the
persisted result. It does not embed runtime IDs or machine settings in the repository.
Use 256 × 256 pixels for a good small sidebar icon. PNGs up to 1024 × 1024 are accepted.
Artwork generation and larger-source export are separate design steps.

Applying an icon that byte-for-byte matches Orca's existing upload is a verified no-op,
and a project settings round-trip preserves all other fields.
