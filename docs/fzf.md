# Search with fzf

Orca Bridge supplies structured session rows; `fzf` supplies the interactive picker.
These examples require `jq` and `fzf`. They read local history without pairing or a
running Orca app. Opening the selection uses the existing `open` identity checks.

## Load once and filter instantly

Add this function to a Bash or Zsh configuration:

```sh
orca-pick() {
  local selected
  selected=$(
    orca-bridge recent --limit 2000 --json \
      | jq -r '.[] | [.id, .title, .agent, .cwd] | @tsv' \
      | fzf --delimiter '\t' --with-nth=2.. --query "${1:-}" \
          --preview 'orca-bridge sessions show {1} --json'
  ) || return
  [ -n "$selected" ] || return
  orca-bridge open "${selected%%$'\t'*}"
}

orca-pick
orca-pick 'migration'
```

Enter opens the selected conversation; Escape cancels. The first, hidden column is
the durable session ID. Preview and open use that ID, never a title or folder.
`jq`'s `@tsv` escapes embedded tabs and newlines so each session stays on one row.
The list is limited to the 2,000 most recently updated sessions; increase `--limit`
for a larger history. Add `--agent codex` or `--path /absolute/project` to `recent`
to narrow the source list. This mode uses fzf's ranking over title, agent and folder.

To return an ID for another command instead of opening it, replace the function's
last line with `printf '%s\n' "${selected%%$'\t'*}"`.

## Run Bridge search as you type

Use this when you want Bridge's title weighting, typo matching, and branch/model
search across the full indexed history. The picker reloads up to 100 matches per
query and preserves Bridge's ranking with `--no-sort`:

```sh
orca-bridge search --limit 100 --json \
  | jq -r '.[] | [.id, .title, .agent, .cwd] | @tsv' \
  | fzf --disabled --no-sort --delimiter '\t' --with-nth=2.. \
      --bind "change:reload:sleep 0.05; orca-bridge search --limit 100 --json -- {q} | jq -r '.[] | [.id, .title, .agent, .cwd] | @tsv'" \
      --preview 'orca-bridge sessions show {1} --json' \
  | cut -f1
```

This prints the selected ID. Use this pipeline as the value of `selected` in the
function above to open on Enter. `fzf` shell-quotes `{q}` and `{1}`; keep those
placeholders unquoted in its command strings. `--` allows queries starting with a
dash. The 50 ms debounce avoids spawning search processes for every rapid keypress;
search itself measured about 9 ms warm on the development Mac. No daemon is needed.
