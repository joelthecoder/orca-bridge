#!/usr/bin/env python3
"""Read-only native resource and local worktree inspection; no raw runtime data is logged."""
import json
import pathlib
import subprocess
import sys

binary = str(pathlib.Path(sys.argv[1]).resolve())
project = pathlib.Path(__file__).resolve().parents[1]


def value(*args):
    result = subprocess.run([binary, *args, '--json'], capture_output=True, text=True, timeout=30)
    if result.returncode:
        raise RuntimeError(f'{args[0]} inspection failed (exit {result.returncode}); inspect it directly')
    return json.loads(result.stdout)


resources = value('resources', '--limit', '2')
assert resources['schemaVersion'] == 1
assert resources['processMemoryMetric'] == 'rss'
assert isinstance(resources['app']['memoryBytes'], int)
assert isinstance(resources['host']['totalMemoryBytes'], int)
assert isinstance(resources['orcaTotal']['memoryBytes'], int)
assert len(resources['terminals']) <= 2
assert all(row['memoryBytes'] is None or row['memoryBytes'] >= 0 for row in resources['terminals'])
assert all(row['binding'] != 'matched' or (row['terminal'] and row['incarnationId']) for row in resources['terminals'])
processes = value('worktrees', 'ps', '--limit', '2')
assert len(processes['worktrees']) <= 2 and 'hostScope' in processes
assert all('preview' not in row and 'agents' not in row for row in processes['worktrees'])
disk = value('worktrees', 'usage', '--worktree', f'path:{project}', '--timeout', '5')
assert len(disk['worktrees']) == 1
row = disk['worktrees'][0]
assert row['disk']['available'] and row['disk']['allocatedBytes'] >= 0
assert row['git']['available'] and isinstance(row['git']['dirty'], bool)
assert row['worktreeSelector']
scoped = value('resources', '--worktree', f'path:{project}', '--sort', 'cpu', '--limit', '2')
assert scoped['selection']['worktreeId'] == row['id']
assert all(terminal['worktreeId'] == row['id'] for terminal in scoped['terminals'])
preview = value('worktrees', 'remove', f'path:{project}', '--dry-run')
assert preview['dryRun'] is True and preview['allowed'] is False
assert any(blocker['code'] == 'main_worktree' for blocker in preview['blockers'])
assert preview['target']['worktreeSelector'] == row['worktreeSelector']
doctor = value('doctor')
features = doctor['capabilities']['features']
assert all(feature['state'] in ('supported', 'unsupported', 'unknown') for feature in features.values())
assert 'promptRetry' in features and 'launchRetry' in features
compact = value('resources', '--fields', 'orcaTotal', '--limit', '1')
assert 'terminals' not in compact and 'orcaTotal' in compact
assert 'terminalCount' in compact and 'terminalsTruncated' in compact
inventory = value('worktrees', 'inventory', '--include-external', '--repo', f'path:{project}')
assert inventory['schemaVersion'] == 1 and inventory['worktrees']
assert 'unavailableRepoCount' in inventory and 'scanLimited' in inventory
assert all(entry['source'] in ('orca-native', 'git-discovered') for entry in inventory['worktrees'])
assert all('detachedHead' in entry and 'prunable' in entry and 'listedByOrca' in entry for entry in inventory['worktrees'])
print(json.dumps({
    'ok': True,
    'checks': ['native-memory-cpu', 'verified-terminal-bindings', 'safe-worktree-process-projection', 'local-allocated-disk', 'read-only-git-status', 'resource-worktree-scope', 'nonmutating-removal-preview', 'capability-evidence', 'compact-coverage-preservation', 'registered-repository-git-inventory'],
    'shownTerminalRows': len(resources['terminals']),
    'nativeBindingsVerified': resources['bindingsVerified'],
    'stoppedTerminals': 0,
    'removedWorktrees': 0,
}, indent=2))
