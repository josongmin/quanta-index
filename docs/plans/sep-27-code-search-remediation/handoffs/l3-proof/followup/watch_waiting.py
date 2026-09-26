import datetime
import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

base = pathlib.Path('/private/tmp/qi-l3-native-evidence')
label = sys.argv[1]
env = dict(os.environ, CARGO_BUILD_JOBS='2', QUANTA_INDEX_RESOURCE_ADMISSION='1')
log = base / (label + '.log')
candidate = base / (label + '-waiting-candidate.json')
while not (base / (label + '-result.json')).exists():
    if b'resource admission: admitted lock=' in log.read_bytes():
        break
    subprocess.run([sys.executable, str(base / 'capture.py'), str(candidate)],
                   stdout=subprocess.DEVNULL, env=env, check=True)
    observed = log.read_bytes()
    # resource_admission.py flushes this marker before executing Cargo. Only
    # snapshots completed before the marker are admissible source baselines.
    if b'resource admission: admitted lock=' in observed:
        break
    target = base / (label + '-waiting.json')
    candidate.replace(target)
    (base / (label + '-waiting.log')).write_bytes(observed)
    (base / (label + '-waiting-boundary.json')).write_text(json.dumps({
        'captured_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'snapshot_sha256': hashlib.sha256(target.read_bytes()).hexdigest(),
        'log_prefix_sha256': hashlib.sha256(observed).hexdigest(),
        'log_prefix_bytes': len(observed),
        'admitted_marker_absent_after_snapshot': True,
        'capture_command': [sys.executable, str(base / 'capture.py'), str(candidate)],
    }, indent=2) + '\n')
    time.sleep(3)
print(json.dumps({'label': label, 'waiting_snapshot_exists': (base / (label + '-waiting.json')).exists()}))
