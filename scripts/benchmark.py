#!/usr/bin/env python3
"""Read-only whole-process benchmarks. No session content is printed."""
import json
import pathlib
import statistics
import subprocess
import sys
import time
binary=str(pathlib.Path(sys.argv[1]).resolve())
results=[]
for command in [['--version'],['recent','--limit','10','--json'],['search','signalbox','--limit','10','--json']]:
    subprocess.run([binary,*command],check=True,stdout=subprocess.DEVNULL)
    measurements=[]
    for _ in range(30):
        start=time.perf_counter()
        subprocess.run([binary,*command],check=True,stdout=subprocess.DEVNULL)
        measurements.append((time.perf_counter()-start)*1000)
    results.append({'command':command,'processMs':{'min':round(min(measurements),2),'median':round(statistics.median(measurements),2),'max':round(max(measurements),2)}})
print(json.dumps(results,indent=2))
