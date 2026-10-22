#!/usr/bin/env python3
"""Read-only live checks, intentionally separate from make check."""
import json
import pathlib
import statistics
import subprocess
import sys
import time

binary = str(pathlib.Path(sys.argv[1]).resolve())

def run(*args, input=None, timeout=15):
    result = subprocess.run([binary, *args], input=input, text=True, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f'{args}: {result.stderr.strip()}')
    return result.stdout

def value(*args):
    return json.loads(run(*args, '--json'))

health = value('doctor')
assert health['history']['ok'] and health['runtime']['ok'], health
sessions = value('recent', '--limit', '3')
assert sessions and len(sessions) <= 3
sid = sessions[0]['id']
assert value('sessions', 'show', sid)['id'] == sid
link = value('sessions', 'link', sid)['url']
assert link.startswith('orca-bridge://session/')
rows = [json.loads(line) for line in run('recent', '--limit', '3', '--jsonl').splitlines()]
assert len(rows) == len(sessions)
filtered = [json.loads(line) for line in run('search', sessions[0]['agent'], '--stdin', '--jsonl', input='\n'.join(json.dumps(row) for row in rows)+'\n').splitlines()]
assert filtered
alfred = json.loads(run('recent', '--limit', '3', '--alfred'))
assert alfred['items'][0]['arg'] == sid
requests = '\n'.join(json.dumps({'id': i, 'method': method}) for i, method in enumerate(['status.get', 'session.tabs.listAll']))+'\n'
responses = [json.loads(line) for line in run('rpc', '--pipe', input=requests).splitlines()]
assert [r['id'] for r in responses] == [0, 1] and all(r['ok'] for r in responses)
assert json.loads(run('filter', 'beta', '--field', '/title', input='{"title":"alpha"}\n{"title":"beta"}\n'))['title'] == 'beta'
with __import__('tempfile').TemporaryDirectory() as state:
    events = [json.loads(line) for line in run('--state-dir', state, 'watch', '--topics', 'sessions,terminals', '--count', '2', '--timeout', '5', '--record').splitlines()]
    assert len(events) == 2 and all(e['kind'] == 'snapshot' for e in events), events
    recorded = json.loads(run('--state-dir', state, 'events', 'list', '--json'))
    assert len(recorded) == 2 and recorded[0]['id'] < recorded[1]['id']
usage = value('sessions', 'usage', sid)
assert usage is not None
# A downstream close must finish successfully, without a broken-pipe diagnostic.
process = subprocess.Popen([binary, 'watch', '--topics', 'sessions', '--interval-ms', '100'], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
assert process.stdout.readline()
process.stdout.close()
process.send_signal(__import__('signal').SIGINT)
assert process.wait(timeout=5) == 0, process.stderr.read().decode()
measurements = []
for _ in range(10):
    start = time.perf_counter()
    run('search', sessions[0]['agent'], '--limit', '10', '--json')
    measurements.append((time.perf_counter()-start)*1000)
print(json.dumps({'ok': True, 'historySessions': health['history']['sessions'], 'nativePaired': health['nativeStream']['paired'], 'checks': ['history', 'fuzzy-search', 'JSONL-input-output', 'Alfred', 'RPC-pipeline', 'arbitrary-JSONL-filter', 'watch-record-replay', 'usage', 'cancellation'], 'searchProcessMs': {'min': round(min(measurements),2), 'median': round(statistics.median(measurements),2), 'max': round(max(measurements),2)}}, indent=2))
