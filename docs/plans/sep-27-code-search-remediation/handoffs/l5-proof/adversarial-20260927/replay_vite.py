import copy
import hashlib
import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path.cwd()))
from tools.benchmark.retrieval import symbol_coverage

out = pathlib.Path('/private/tmp/quanta-index-l5-adversarial-20260927')
base = pathlib.Path('/private/tmp/quanta-index-l5-completion/frozen-vite-runtime')
manifest = pathlib.Path('/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-external-check/manifests/vite.json')
corpus = json.loads(manifest.read_bytes())
phase_path = base / 'phase-metrics.json'
metrics = json.loads(phase_path.read_bytes())
artifact = symbol_coverage.verify_artifact(metrics, phase_path, corpus)
report = json.loads(artifact.read_bytes())
failed = [row for row in report['preflight']['files'] if row['coverage']['state'] == 'parse_failed']
assert len(failed) == 1 and len(failed[0]['diagnostics']) > 0
assert metrics['file_count'] == 256
with tempfile.TemporaryDirectory() as directory:
    directory = pathlib.Path(directory)
    failed[0].update(diagnostics=[], diagnostics_truncated=True)
    altered = directory / artifact.name
    altered.write_text(json.dumps(report))
    candidate = copy.deepcopy(metrics)
    candidate['symbol_preflight_sha256'] = hashlib.sha256(altered.read_bytes()).hexdigest()
    try:
        symbol_coverage.verify_artifact(candidate, directory / phase_path.name, corpus)
    except ValueError as error:
        assert 'diagnostic retention' in str(error), str(error)
        refusal = str(error)
    else:
        raise AssertionError('an omitted real Vite diagnostic was accepted')
result = {
    'status': 'VERIFIED',
    'scope': 'Current consumer replay of prior frozen 256-file Vite preflight; not a fresh producer/process run',
    'files': 256,
    'omitted_diagnostic_refusal': refusal,
    'inputs': {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in [manifest, phase_path, artifact]},
}
(out / 'vite-replay-result.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
