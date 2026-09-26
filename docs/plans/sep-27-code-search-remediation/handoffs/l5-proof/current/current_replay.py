import hashlib
import json
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path.cwd()))
from tools.benchmark.retrieval import proof_inventory, sdk_proof

base = pathlib.Path('/private/tmp/quanta-index-l5-completion')
sdk = base / 'sdk-final'
runtime = base / 'frozen-vite-runtime'
for role, name in [('sdk', 'inventory.json'), ('rust', 'rust-inventory.json')]:
    proof_inventory.verify_inventory_authority(sdk / name, role)
result = sdk_proof.build_summary(
    record_path=sdk / 'actual-runner-record.json',
    nextest_path=sdk / 'nextest.jsonl',
    runner_path=runtime / 'quanta-index-retrieval-bench',
    inventory_path=sdk / 'inventory.json',
    searchd_path=runtime / 'quanta-index-searchd',
)
assert result == json.loads((sdk / 'sdk-results.json').read_text())
sources = {}
for path in ['tools/benchmark/retrieval/contract_proof.py', 'tools/benchmark/retrieval/portable_proof.py']:
    raw = pathlib.Path(path).read_bytes()
    assert '--test l5_parser_regressions' in raw.decode()
    sources[path] = hashlib.sha256(raw).hexdigest()
output = {'status': 'VERIFIED', 'scope': 'Current consumer compatibility with frozen raw SDK evidence and exact binaries; source snapshots remain separate', 'source_sha256': sources, 'sdk_summary': result}
(base / 'current-replay-result.json').write_text(json.dumps(output, indent=2) + '\n')
print(json.dumps(output))
