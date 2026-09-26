import datetime
import gzip
import hashlib
import json
import pathlib
import re
import shutil
import subprocess
import sys

root = pathlib.Path('/Users/songmin/Documents/code-new/quanta-index')
temp = pathlib.Path('/private/tmp/qi-l3-native-evidence')
handoff = root / 'docs/plans/sep-27-code-search-remediation/handoffs'
dest = handoff / 'l3-proof/followup'
dest.mkdir(parents=True, exist_ok=True)
final_label = sys.argv[1]
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
owned = ['crates/quanta-index-lexical/src/' + p for p in
         ['budgeted_search.rs', 'ranked_page.rs', 'ranked_page_tests.rs']]
roots = json.loads((temp / 'lexical-path-dependency-closure.json').read_text())
front = ['Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'Justfile',
         'tools/ci/resource_admission.py', 'tools/benchmark/producer_execution.py']


def relevant(path):
    return (path in front or path.startswith(('.cargo/', 'scripts/', 'vendor/tantivy-sstable/'))
            or any(path.startswith(r + '/') for r in roots))


def archive(src, name=None, compress=False):
    target = dest / (name or src.name)
    if compress:
        target = target.with_name(target.name + '.gz')
        target.write_bytes(gzip.compress(src.read_bytes(), mtime=0))
    else:
        shutil.copyfile(src, target)
    value = {'path': str(target.relative_to(root)), 'sha256': sha(target)}
    if compress:
        value['uncompressed_sha256'] = sha(src)
    return value


subprocess.run([sys.executable, str(temp / 'capture.py'), str(temp / 'audit2-current.json')],
               cwd=root, check=True)
current = json.loads((temp / 'audit2-current.json').read_text())
baseline = json.loads((temp / 'audit2-prepatch.json').read_text())
metadata_basis = json.loads((temp / 'rr-owner-final-4-after.json').read_text())
manifest_paths = ['Cargo.toml', 'Cargo.lock'] + [r + '/Cargo.toml' for r in roots]
assert all(current['files'][p] == metadata_basis['files'][p] for p in manifest_paths)
selected = {p: digest for p, digest in current['files'].items() if relevant(p)}
selected_digest = hashlib.sha256(json.dumps(selected, sort_keys=True, separators=(',', ':')).encode()).hexdigest()

runs = []
for path in sorted(temp.glob('audit2-*-result.json')):
    label = path.name.removesuffix('-result.json')
    receipt = json.loads(path.read_text())
    before = json.loads((temp / f'{label}-before.json').read_text())
    after = json.loads((temp / f'{label}-after.json').read_text())
    assert receipt['log_sha256'] == sha(temp / f'{label}.log')
    source_before = before
    waiting_artifacts = []
    waiting = temp / f'{label}-waiting.json'
    if waiting.exists():
        boundary = json.loads((temp / f'{label}-waiting-boundary.json').read_text())
        prefix = (temp / f'{label}-waiting.log').read_bytes()
        assert sha(waiting) == boundary['snapshot_sha256']
        assert hashlib.sha256(prefix).hexdigest() == boundary['log_prefix_sha256']
        assert b'resource admission: admitted lock=' not in prefix
        assert (temp / f'{label}.log').read_bytes().startswith(prefix)
        source_before = json.loads(waiting.read_text())
        waiting_artifacts = [archive(waiting, compress=True)] + [
            archive(temp / f'{label}-waiting{suffix}') for suffix in ['.log', '-boundary.json']]
    # Cargow/config/front-door scripts can be consumed before queue admission.
    # Cargo source can only be consumed after the flushed admitted marker.
    front_drift = [p for p in receipt['changed_inputs']
                   if p in front or p.startswith(('.cargo/', 'scripts/'))]
    execution_drift = [p for p in source_before['files'].keys() | after['files'].keys()
                       if relevant(p) and source_before['files'].get(p) != after['files'].get(p)]
    drift = sorted(set(front_drift + execution_drift))
    stale = sorted(p for p in after['files'].keys() | current['files'].keys()
                   if relevant(p) and after['files'].get(p) != current['files'].get(p))
    status = ('FAILED' if receipt['exit_code'] else
              'BLOCKED' if drift or stale or current['head'] != after['head'] else 'VERIFIED')
    raw = (temp / f'{label}.log').read_text()
    counts = [dict(zip(['passed', 'failed', 'ignored', 'measured', 'filtered'], map(int, m)))
              for m in re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out', raw)]
    artifacts = [archive(temp / f'{label}-{suffix}.json', compress=True) for suffix in ['before', 'after']]
    artifacts += [archive(temp / f'{label}{suffix}') for suffix in ['.log', '-result.json']]
    artifacts += waiting_artifacts
    runs.append({'id': label, **receipt, 'status_for_current_source': status,
                 'terminal_test_counts': counts, 'relevant_drift_during_run': drift,
                 'relevant_changes_since_run': stale, 'artifacts': artifacts,
                 'source_baseline': 'last snapshot completed while the admission marker was still absent' if waiting.exists() else 'command launch',
                 'source_sha256_at_baseline': source_before['source_sha256']})
final = next(r for r in runs if r['id'] == final_label)
assert final['status_for_current_source'] == 'VERIFIED', final
assert len(final['terminal_test_counts']) == 5, final['terminal_test_counts']
assert all(c['failed'] == c['ignored'] == c['measured'] == c['filtered'] == 0
           for c in final['terminal_test_counts']), final['terminal_test_counts']
assert sum(c['passed'] for c in final['terminal_test_counts']) == 188
assert len(final['binary_sha256']) == 5, final['binary_sha256']
assert all(sha(pathlib.Path(p)) == digest for p, digest in final['binary_sha256'].items())

checks = []
for label, command in [
    ('format', ['rustfmt', '--edition', '2024', '--config', 'skip_children=true', '--check', *owned]),
    ('diff', ['git', 'diff', '--check']),
]:
    output = dest / f'audit2-static-{label}.log'
    with output.open('wb') as stream:
        run = subprocess.run(command, cwd=root, stdout=stream, stderr=subprocess.STDOUT)
    checks.append({'command': command, 'exit_code': run.returncode,
                   'status': 'VERIFIED' if run.returncode == 0 else 'FAILED',
                   'artifact': str(output.relative_to(root)), 'sha256': sha(output)})
assert all(c['status'] == 'VERIFIED' for c in checks), checks

source_artifacts = []
for p in owned:
    source_artifacts.append(archive(root / p, 'final-' + pathlib.Path(p).name))
support = [archive(temp / name, compress=name.endswith('.json')) for name in
           ['audit2-prepatch.json', 'audit2-current.json', 'capture.py', 'run.py',
            'finalize_followup.py', 'watch_waiting.py', 'lexical-path-dependency-closure.json', 'rr-metadata.json']]
prior_artifacts = []
for name in ['L3_HANDOFF.md', 'L3_HANDOFF.source.json']:
    target = dest / ('prior-' + name)
    if not target.exists():
        shutil.copyfile(handoff / name, target)
    prior_artifacts.append({'path': str(target.relative_to(root)), 'sha256': sha(target)})

value = {
    'captured_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'state': 'VERIFIED',
    'scope': 'Two follow-up L3 repairs and selected lexical library/integration tests on a shared dirty source',
    'head': current['head'], 'dirty': current['dirty'],
    'owned_files_before': {p: baseline['files'][p] for p in owned},
    'owned_files_after': {p: current['files'][p] for p in owned},
    'current_source_sha256': current['source_sha256'],
    'selected_input_sha256': selected_digest, 'selected_inputs': selected,
    'path_dependency_closure': roots,
    'dependency_graph_note': 'Cargo metadata graph reused only after revalidating root manifest, lock and all closure manifests against its recorded source snapshot.',
    'dependency_manifests': {p: current['files'][p] for p in manifest_paths},
    'environment': current['environment'], 'platform': current['platform'], 'rustc': current['rustc'],
    'resource_admission': 'Canonical cargow shared build-test.lock; QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2',
    'repairs': [
        {'id': 'preinterrupted-request', 'status': 'VERIFIED',
         'cause': 'Initial fruit-byte reservation and query weight preparation preceded the first cancellation/deadline observation.',
         'change': 'Observe the request before collection allocation or weight construction.',
         'oracle': 'Cancellation/deadline error identity; instrumented weight and scorer opens plus ledger work/peak/resident all remain zero.',
         'red_run': 'audit2-cancel-red-2',
         'test': 'l3_preinterrupted_collection_refuses_before_weight_or_allocation'},
        {'id': 'duplicate-numeric-order-key', 'status': 'VERIFIED',
         'cause': 'Column.first(doc) accepted malformed duplicate start_line/end_line values.',
         'change': 'Require exactly one numeric ordering value, matching string ordering-field cardinality checks.',
         'oracle': 'Real malformed Tantivy document must return Storage before advancing to the next hit; both fields and ranked/grouped collectors.',
         'scope': 'Malformed stored-index rejection; not a reproduced defect in validated ingestion.',
         'red_run': 'audit2-lines-red',
         'test': 'l3_duplicate_line_keys_are_rejected_by_ranked_and_grouped_collectors'},
    ],
    'accepted_run': final_label, 'passed': 188, 'failed': 0,
    'runs': runs, 'static_checks': checks,
    'source_artifacts': source_artifacts, 'supporting_artifacts': support,
    'prior_handoff_artifacts': prior_artifacts,
    'communication': 'No inter-task reading, sending, waiting or subagents',
    'requested_claims': {
        'two_reproduced_defects_repaired': 'VERIFIED',
        'selected_lexical_tests': 'VERIFIED',
        'current_SDK_CLI_tests': 'NOT_RUN',
        'whole_repository_CI': 'NOT_RUN',
        'installed_daemon_E2E': 'NOT_RUN',
        'request_wide_RSS_and_performance': 'NOT_RUN',
    },
    'exclusions': ['Unselected tests and SDK/CLI after concurrent contract changes',
                   'Installed process/E2E, release/deployment/activation qualification',
                   'Complete request memory/RSS, ranking-quality and latency measurements'],
    'provenance_note': 'Only the accepted run contributes to 188. Failed reproductions, compile failures and source-drifted passes are retained separately. Hashes are local source binding, not signed independent attestation.',
}
receipt_path = handoff / 'L3_FOLLOWUP.source.json'
receipt_path.write_text(json.dumps(value, indent=2) + '\n')
report = f'''# L3 follow-up code audit

State: **VERIFIED for the two repairs and selected lexical tests only**.
HEAD: `{current['head']}`, shared dirty `main`.
Selected source/input SHA256: `{selected_digest}`.
Receipt SHA256: `{sha(receipt_path)}` in `L3_FOLLOWUP.source.json`.

## Reproduced defects and repairs

1. An already cancelled/expired request could allocate the fruit buffer or build
   the query weight before checking interruption. A one-byte collection limit
   masked cancellation with a resource-budget error. `search_with_probe` now
   checks interruption first. The regression requires typed cancellation/deadline
   with zero weight/scorer opens and zero resource work/peak/resident bytes at
   both tight and ample byte limits.
2. Numeric order fields accepted their first value even when a malformed index
   document held multiple `start_line`/`end_line` values. Both fields now require
   exactly one value. Real malformed Tantivy fixtures assert Storage refusal
   without advancing in both ranked and grouped collectors. This is stored-index
   integrity hardening, not a demonstrated normal-ingestion failure.

Owned changes in this continuation are confined to `budgeted_search.rs`,
`ranked_page.rs` and `ranked_page_tests.rs`. Other shared edits were preserved.
The cancellation tick comment now states scorer-action granularity; no inner
engine wall-clock bound is asserted.

## Verification

Environment: `QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2`.

```sh
{' '.join(final['command'])}
```

Result: **188 passed, 0 failed, 0 ignored**: library 170,
`cancellation_inside_search` 3, `execution_budget` 4, `l3_exact_source` 6,
`ranked_pages` 5. Run: `{final_label}`. Source baseline:
{final['source_baseline']}. Relevant source was stable from that baseline before
Cargo execution through the final snapshot. Front-door/configuration inputs
were stable from command launch, including queue admission. Any earlier changes
while queued are retained in the receipt and are not execution-window drift.
Raw log SHA256: `{final['log_sha256']}`.

Formatting for the three owned files and `git diff --check` are **VERIFIED**.
The receipt includes complete dirty/source inventories, dependency closure,
environment/toolchain identity, exact commands, five binary digests and archived
raw outputs. Red reproductions are `audit2-cancel-red-2` and `audit2-lines-red`;
each executed one failing test before its repair. Earlier compile failures and
source-drifted passing runs remain historical diagnostics and are not counted.

SDK/CLI on the concurrently changed contract, full repository CI, installed
daemon E2E, request-wide memory/RSS and ranking/latency performance are **NOT_RUN**.
The previous 373-test result belongs to an earlier snapshot; its original
handoff and receipt are archived under `l3-proof/followup/prior-L3_HANDOFF.*`.
No inter-task messages were read or sent. No commit/push/reset was performed.
'''
(handoff / 'L3_FOLLOWUP_AUDIT.md').write_text(report)
old_text = (dest / 'prior-L3_HANDOFF.md').read_text()
prefix = '''# Current L3 follow-up

The current continuation is [L3_FOLLOWUP_AUDIT.md](L3_FOLLOWUP_AUDIT.md), with
188 selected lexical tests and two additional repaired defects. Its exact source
and raw evidence are bound by [L3_FOLLOWUP.source.json](L3_FOLLOWUP.source.json).
The 373-test result below belongs to an earlier source snapshot and does not
qualify the current SDK/CLI or whole repository.

---

## Historical handoff (superseded source snapshot)

'''
(handoff / 'L3_HANDOFF.md').write_text(prefix + old_text)
old = json.loads((dest / 'prior-L3_HANDOFF.source.json').read_text())
old['historical_execution_state'] = old['state']
old['state'] = 'BLOCKED'
old['state_reason'] = 'Historical source is superseded; this receipt is not current-source qualification.'
old['superseded_by'] = {'path': str(receipt_path.relative_to(root)), 'sha256': sha(receipt_path)}
(handoff / 'L3_HANDOFF.source.json').write_text(json.dumps(old, indent=2) + '\n')
assert all(sha(root / p) == digest for p, digest in selected.items()), 'source changed while recording evidence'
subprocess.run(['git', 'diff', '--check'], cwd=root, check=True)
print(json.dumps({'accepted_run': final_label, 'passed': 188,
                  'selected_input_sha256': selected_digest,
                  'receipt_sha256': sha(receipt_path), 'report': str(handoff / 'L3_FOLLOWUP_AUDIT.md')}))
