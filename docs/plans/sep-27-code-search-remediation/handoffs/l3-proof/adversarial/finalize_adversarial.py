import datetime
import difflib
import gzip
import hashlib
import json
import pathlib
import shutil
import subprocess
import sys

root = pathlib.Path('/Users/songmin/Documents/code-new/quanta-index')
temp = pathlib.Path('/private/tmp/qi-l3-native-evidence')
handoff = root / 'docs/plans/sep-27-code-search-remediation/handoffs'
dest = handoff / 'l3-proof/adversarial'
dest.mkdir(parents=True, exist_ok=True)
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
final_label = sys.argv[1]
roots = json.loads((temp / 'audit3-path-closure.json').read_text())
front = {'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'Justfile',
         'tools/ci/resource_admission.py', 'tools/benchmark/producer_execution.py'}
tests = {'execution_budget.rs', 'ranked_pages.rs', 'l3_exact_source.rs', 'cancellation_inside_search.rs'}
owned = ['crates/quanta-index-lexical/src/' + p for p in ['ranked_page.rs', 'ranked_page_tests.rs']]


def relevant(path):
    if path in front or path.startswith(('.cargo/', 'scripts/', 'vendor/tantivy-sstable/')):
        return True
    for package in roots:
        prefix = package + '/'
        if path.startswith(prefix):
            child = path[len(prefix):]
            if child.startswith('tests/'):
                return package == 'crates/quanta-index-lexical' and (
                    child[6:] in tests or child.startswith('tests/support/'))
            return True
    return False


def archive(path, name=None):
    target = dest / (name or path.name)
    if path.suffix == '.json':
        target = target.with_name(target.name + '.gz')
        target.write_bytes(gzip.compress(path.read_bytes(), mtime=0))
    else:
        shutil.copyfile(path, target)
    return {'path': str(target.relative_to(root)), 'sha256': sha(target), 'original_sha256': sha(path)}


subprocess.run([sys.executable, str(temp / 'capture.py'), str(temp / 'audit3-current.json')], cwd=root, check=True)
current = json.loads((temp / 'audit3-current.json').read_text())
baseline = json.loads((temp / 'audit3-prepatch.json').read_text())
graph_source = json.loads((temp / 'audit3-metadata-source.json').read_text())
graph_before = json.loads((temp / 'audit3-metadata-before.json').read_text())
manifests = ['Cargo.toml', 'Cargo.lock'] + [p + '/Cargo.toml' for p in roots]
assert all(current['files'][p] == graph_source['files'][p] for p in manifests)
assert all(graph_before['files'][p] == graph_source['files'][p] for p in manifests)
selected = {p: d for p, d in current['files'].items() if relevant(p)}
selected_sha = hashlib.sha256(json.dumps(selected, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
runs = []
import re
for file in sorted(temp.glob('audit3-*-result.json')):
    label = file.name.removesuffix('-result.json')
    result = json.loads(file.read_text())
    start = json.loads((temp / f'{label}-before.json').read_text())
    end = json.loads((temp / f'{label}-after.json').read_text())
    raw = (temp / f'{label}.log').read_bytes()
    assert hashlib.sha256(raw).hexdigest() == result['log_sha256']
    bound = start
    source_boundary = 'command launch'
    artifacts = [archive(temp / f'{label}{suffix}') for suffix in ['-before.json', '-after.json', '-result.json', '.log']]
    waiting = temp / f'{label}-waiting.json'
    if waiting.exists():
        marker = json.loads((temp / f'{label}-waiting-boundary.json').read_text())
        prefix = (temp / f'{label}-waiting.log').read_bytes()
        assert sha(waiting) == marker['snapshot_sha256']
        assert hashlib.sha256(prefix).hexdigest() == marker['log_prefix_sha256']
        assert raw.startswith(prefix) and b'resource admission: admitted lock=' not in prefix
        if result['changed_inputs']:
            bound = json.loads(waiting.read_text())
            source_boundary = 'last completed snapshot before the flushed Cargo admission marker'
        artifacts += [archive(temp / f'{label}{suffix}') for suffix in ['-waiting.json', '-waiting-boundary.json', '-waiting.log']]
    drift = sorted(p for p in bound['files'].keys() | end['files'].keys()
                   if relevant(p) and bound['files'].get(p) != end['files'].get(p))
    front_drift = [p for p in result['changed_inputs'] if p in front or p.startswith(('.cargo/', 'scripts/'))]
    stale = sorted(p for p in end['files'].keys() | selected.keys()
                   if relevant(p) and end['files'].get(p) != selected.get(p))
    counts = [dict(zip(['passed', 'failed', 'ignored', 'measured', 'filtered'], map(int, m))) for m in
              re.findall(rb'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out', raw)]
    status = 'FAILED' if result['exit_code'] else 'BLOCKED' if drift or front_drift or stale or current['head'] != end['head'] else 'VERIFIED'
    runs.append({'id': label, **result, 'status_for_current_source': status,
                 'source_boundary': source_boundary, 'source_sha256_at_boundary': bound['source_sha256'],
                 'execution_input_drift': drift, 'front_door_drift': front_drift,
                 'changes_since_execution': stale, 'counts': counts, 'artifacts': artifacts})
final = next(r for r in runs if r['id'] == final_label)
assert final['status_for_current_source'] == 'VERIFIED', final
assert len(final['counts']) == len(final['binary_sha256']) == 5
assert all(c['failed'] == c['ignored'] == c['measured'] == c['filtered'] == 0 for c in final['counts'])
passed = sum(c['passed'] for c in final['counts'])
assert passed == 190, passed

checks = []
for label, command in [('format', ['rustfmt', '--edition', '2024', '--config', 'skip_children=true', '--check', *owned]),
                       ('diff', ['git', 'diff', '--check'])]:
    log = dest / f'static-{label}.log'
    with log.open('wb') as stream:
        check = subprocess.run(command, cwd=root, stdout=stream, stderr=subprocess.STDOUT)
    checks.append({'command': command, 'exit_code': check.returncode, 'status': 'VERIFIED' if check.returncode == 0 else 'FAILED',
                   'path': str(log.relative_to(root)), 'sha256': sha(log)})
assert all(c['exit_code'] == 0 for c in checks), checks
patch = []
for p in owned:
    name = pathlib.Path(p).name
    archive(temp / 'audit3-baseline' / name, 'before-' + name)
    archive(root / p, 'after-' + name)
    patch.extend(difflib.unified_diff((temp / 'audit3-baseline' / name).read_text().splitlines(True),
                                    (root / p).read_text().splitlines(True), fromfile='a/' + p, tofile='b/' + p))
(dest / 'owned.patch').write_text(''.join(patch))
support = [archive(temp / name) for name in ['audit3-prepatch.json', 'audit3-current.json', 'capture.py', 'run.py',
                                           'watch_waiting.py', 'finalize_adversarial.py', 'audit3-path-closure.json',
                                           'audit3-metadata.json', 'audit3-metadata-before.json', 'audit3-metadata-source.json']]
for name in ['L3_HANDOFF.md', 'L3_HANDOFF.source.json']:
    target = dest / ('prior-' + name)
    if not target.exists():
        shutil.copyfile(handoff / name, target)

value = {'captured_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'state': 'VERIFIED',
         'scope': 'L3 grouped-harvest error propagation and selected lexical correctness tests',
         'head': current['head'], 'dirty': current['dirty'],
         'selected_input_sha256': selected_sha, 'selected_inputs': selected,
         'source_scope': 'All files in the lexical local dependency closure except unselected integration tests; includes selected lexical tests/support, manifests/lock, toolchain, config, scripts, resource-admission owners and vendor SSTable.',
         'path_dependency_closure': roots,
         'metadata_command': ['./scripts/cargow', '--lane', 'test-fast-lane', 'metadata', '--format-version', '1', '--locked'],
         'metadata_note': 'Fresh Cargo metadata; root manifest, lock and every closure manifest were stable during graph capture and revalidated before closeout.',
         'owned_before': {p: baseline['files'][p] for p in owned}, 'owned_after': {p: current['files'][p] for p in owned},
         'environment': current['environment'], 'platform': current['platform'], 'rustc': current['rustc'],
         'finding': {'priority': 'P2', 'status': 'VERIFIED', 'id': 'group-harvest-error-masking',
                     'trigger': 'A grouped representative key fails decoding during harvest in the first of multiple native segments.',
                     'before': 'The next segment was visited; a tight work budget replaced the original Storage error with LexicalCollectionBudgetExceeded.',
                     'after': 'Failed harvest aborts the shared collection. The original Storage error is returned without opening another scorer.',
                     'proof': 'Real two-segment Tantivy index; malformed UTF-8 key with unchanged column layout. Work caps 3 and 100 assert first error identity, exactly one scorer, work 3 and resident bytes 0.',
                     'boundary': 'Native collector fixture below sealed-generation admission; no claim that malformed bytes bypass a generation seal.'},
         'additional_oracle': {'test': 'l3_adversarial_pages_and_groups_match_fixed_score_and_tie_oracles',
                               'page_walks': 24, 'group_cases': 6,
                               'dimensions': '3 segments, score ties, boosts 0/1/2.5, count on/off, page widths 1/2/4/8, path/repo grouping, Unicode keys, lease release.'},
         'accepted_run': final_label, 'passed': passed, 'failed': 0, 'runs': runs, 'static_checks': checks,
         'supporting_artifacts': support, 'owned_patch': {'path': str((dest / 'owned.patch').relative_to(root)), 'sha256': sha(dest / 'owned.patch')},
         'claims': {'native_error_propagation_repair': 'VERIFIED', 'selected_lexical_tests': 'VERIFIED',
                    'current_SDK_CLI_tests': 'NOT_RUN', 'whole_repository_CI': 'NOT_RUN',
                    'installed_daemon_E2E': 'NOT_RUN', 'performance_RSS_ranking_quality': 'NOT_RUN'},
         'communication': 'No inter-task sending, reading, waiting or subagents',
         'provenance_note': 'Only the final selected run contributes to the pass count. Local source/binary/artifact hashes are bindings, not signed independent attestations.'}
receipt = handoff / 'L3_ADVERSARIAL.source.json'
receipt.write_text(json.dumps(value, indent=2) + '\n')
report = f'''# L3 adversarial code audit

**VERIFIED** for one repaired defect and the selected lexical test scope.
HEAD `{current['head']}`, shared dirty `main`.
Selected input SHA256: `{selected_sha}`.
Receipt: `L3_ADVERSARIAL.source.json`, SHA256 `{sha(receipt)}`.

## Reproduced P2 defect

`GroupedPageSegment::harvest` returned a decoding failure without stopping the
shared collection. The wrapper could continue into a later segment, whose work
refusal replaced the original Storage error with `LexicalCollectionBudgetExceeded`.
Harvest now marks the collection aborted on error, preserving the first failure.

The regression mutates a real native two-segment Tantivy index so the first
representative key fails UTF-8 decoding only at harvest. Before the fix, work
limit 3 returned the wrong typed budget error (`audit3-harvest-red`, one executed
failing test). After the fix, limits 3 and 100 preserve Storage, open exactly one
scorer, consume three work units and release all byte reservations. This tests
the native collector below sealed-generation admission; it does not demonstrate
a corrupted file bypassing the generation seal.

The only production change this turn is the harvest abort propagation in
`ranked_page.rs`. Tests are in `ranked_page_tests.rs`; `l3-proof/adversarial/owned.patch`
isolates this turn from earlier shared changes.

## Adversarial oracle and validation

A separate fixed golden tests 24 complete page walks and six grouping cases:
three segments, score ties, boosts 0/1/2.5, count on/off, page widths 1/2/4/8,
path/repo projections, Unicode keys and retained-byte release. It does not derive
the expected ordering from the production comparator. These are subcases of one
test, not additional terminal test counts. Focused L3 result: 31 passed.

Environment: `QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2`.

```sh
{' '.join(final['command'])}
```

Final result: **{passed} passed, 0 failed, 0 ignored**, across the library and four
selected integration targets. Run `{final_label}`. Source boundary:
{final['source_boundary']}; selected inputs remained stable through completion
and matched the final snapshot. Raw log SHA256 `{final['log_sha256']}`.
Formatting and `git diff --check` are **VERIFIED**.

The receipt archives before/after manifests, raw outputs, exact commands, five
binary digests, environment/toolchain identity, the red reproduction and earlier
executions. Unselected integration tests and unrelated concurrent edits are
outside the bound selected scope. Previous 188/373-test receipts are historical
snapshots, not current proof after this change.

Current SDK/CLI, full repository CI, installed daemon E2E, ranking quality,
latency and request-wide RSS are **NOT_RUN**. No score or ranking-weight change,
commit/push/reset, or inter-task communication was performed.
'''
(handoff / 'L3_ADVERSARIAL_AUDIT.md').write_text(report)
(handoff / 'L3_HANDOFF.md').write_text('''# Current L3 handoff

The current code audit and source binding are in
[L3_ADVERSARIAL_AUDIT.md](L3_ADVERSARIAL_AUDIT.md) and
[L3_ADVERSARIAL.source.json](L3_ADVERSARIAL.source.json).

Historical scope and earlier repairs remain in
[L3_FOLLOWUP_AUDIT.md](L3_FOLLOWUP_AUDIT.md),
[L3_FOLLOWUP.source.json](L3_FOLLOWUP.source.json), and
`l3-proof/adversarial/prior-L3_HANDOFF.*`. Earlier test counts must not be combined
with the current run or promoted to current SDK/CLI or full repository proof.
''')
(handoff / 'L3_HANDOFF.source.json').write_text(json.dumps({
    'state': 'VERIFIED', 'scope': value['scope'], 'head': current['head'],
    'dirty': current['dirty'], 'selected_input_sha256': selected_sha,
    'current_receipt': {'path': str(receipt.relative_to(root)), 'sha256': sha(receipt)},
    'historical_receipt': {'path': str((handoff / 'L3_FOLLOWUP.source.json').relative_to(root)),
                           'sha256': sha(handoff / 'L3_FOLLOWUP.source.json'), 'status_for_current_source': 'BLOCKED'},
    'claims': value['claims'],
}, indent=2) + '\n')
assert all(sha(root / p) == d for p, d in selected.items()), 'source drift while archiving'
subprocess.run(['git', 'diff', '--check'], cwd=root, check=True)
print(json.dumps({'run': final_label, 'passed': passed, 'selected_input_sha256': selected_sha, 'receipt_sha256': sha(receipt)}))
