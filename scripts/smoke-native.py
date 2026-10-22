#!/usr/bin/env python3
"""Native live verification. --exercise creates and closes ONE disposable terminal."""
import argparse
import json
import pathlib
import select
import signal
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('binary')
parser.add_argument('--exercise', action='store_true')
args = parser.parse_args()
binary = str(pathlib.Path(args.binary).resolve())

def run(*argv):
    result = subprocess.run([binary, *argv], capture_output=True, text=True, timeout=20)
    if result.returncode:
        raise RuntimeError(result.stderr.strip())
    return json.loads(result.stdout)

checks = []
for method in ['session.tabs.subscribeAll', 'agentSession.subscribeStatus', 'notifications.subscribe']:
    row = run('stream', 'subscribe', method, '--count', '1', '--timeout', '5')
    assert row['frame']['ok'] is True and row['frame']['streaming'] is True
    checks.append({'method':method, 'initialType':row['frame']['result']['type']})

if args.exercise:
    process = subprocess.Popen([binary, 'stream', 'subscribe', 'session.tabs.subscribeAll', '--timeout', '20'],
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
    handle = None
    def read_frame(deadline):
        remaining = deadline-time.monotonic()
        if remaining <= 0 or not select.select([process.stdout], [], [], remaining)[0]:
            raise TimeoutError('Native fixture event timed out')
        line = process.stdout.readline()
        if not line:
            raise RuntimeError('Native stream closed before fixture event')
        return json.loads(line)['frame']['result']
    def contains(value, target):
        if isinstance(value, dict):
            return any(contains(x,target) for x in value.values())
        if isinstance(value, list):
            return any(contains(x,target) for x in value)
        return value == target
    try:
        initial = read_frame(time.monotonic()+5)
        assert initial['type'] == 'snapshots'
        terminal = run('terminals', 'create', '--worktree', 'path:'+str(pathlib.Path.cwd()),
                       '--title', 'Orca Bridge disposable stream smoke', '--command',
                       "printf 'orca-bridge-stream-fixture\\n'", '--json')['terminal']
        handle = terminal['handle']
        tab_id = terminal['tabId']
        deadline = time.monotonic()+5
        while True:
            frame = read_frame(deadline)
            if contains(frame.get('tabs', []), tab_id):
                worktree = frame['worktree']
                break
        run('orca', '--', 'terminal', 'close', '--terminal', handle, '--tab')
        handle = None
        deadline = time.monotonic()+5
        while True:
            frame = read_frame(deadline)
            if frame.get('worktree') == worktree and not contains(frame.get('tabs', []), tab_id):
                break
        checks.append({'fixture':'terminal-tab-created-and-removed', 'ok':True})
    finally:
        if handle:
            run('orca', '--', 'terminal', 'close', '--terminal', handle, '--tab')
        process.send_signal(signal.SIGINT)
        assert process.wait(timeout=12) == 0, 'Native subscriber did not cancel successfully'
        assert not process.stderr.read(), 'Native subscriber emitted diagnostics'
    checks.append({'fixture':'bounded-cancellation', 'ok':True})
print(json.dumps({'ok':True,'checks':checks},indent=2))
