from pathlib import Path
import datetime,difflib,hashlib,json,re,shutil,subprocess,xml.etree.ElementTree as ET
out=Path('/private/tmp/quanta-index-l5-definition-audit-20260927')
root=Path('/Users/songmin/Documents/code-new/quanta-index')
snapshot=Path('/Users/songmin/.codex/worktrees/l5-definition-final/quanta-index')
handoff=root/'docs/plans/sep-27-code-search-remediation/handoffs'
dest=handoff/'l5-proof/definition-audit-20260927'
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
load=lambda p:json.loads(p.read_text())
required=load(snapshot/'benchmarks/retrieval/proof-required-tests.json')
receipts={name:load(out/f'{name}.json') for name in ['rust-final','python-final','vite-final','format-final']}
for name,a in receipts.items():
 assert a['exit_code']==0 and a['changed_inputs']==[],name
 assert a['head']=='681dc3f0c5232b4c756c9fd296d305677560370a',name
 assert a['before']==receipts['rust-final']['before'],name
 assert a['log_sha256']==sha(out/f'{name}.log')
current=None;passed=[];bins=[]
for line in (out/'rust-final.log').read_text().splitlines():
 if 'Running unittests src/lib.rs' in line:current='quanta_index_retrieval_bench'
 elif 'Running unittests src/main.rs' in line:current='binary'
 elif 'Running tests/chunking_contract.rs' in line:current='chunking_contract'
 elif 'Running tests/l5_parser_regressions.rs' in line:current='l5_parser_regressions'
 m=re.fullmatch(r'test (.+) \.\.\. ok',line)
 if m:
  assert current is not None
  if current=='binary':bins.append(m[1])
  else:passed.append('quanta-index-retrieval-bench::'+current+'$'+m[1])
assert sorted(passed)==required['rust'] and len(passed)==135 and len(bins)==3
cases=list(ET.parse(out/'python-final.junit.xml').iter('testcase'))
assert len(cases)==16 and all(not any(c.tag in ('error','failure','skipped') for c in case) for case in cases)
summary={'rust':{'passed':138,'registered':135,'registered_identities':sorted(passed),'binary_tests':bins},'python':{'passed':16,'identities':[c.attrib['classname']+'.'+c.attrib['name'] for c in cases]}}
(out/'test-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
own=['benchmarks/retrieval/src/symbols.rs','benchmarks/retrieval/tests/l5_parser_regressions.rs','benchmarks/retrieval/proof-required-tests.json']
for name in own:assert sha(root/name)==sha(snapshot/name),name
patch=[]
for name in own:
 before=out/('before-'+Path(name).name)
 patch.extend(difflib.unified_diff(before.read_text().splitlines(True),(snapshot/name).read_text().splitlines(True),fromfile='before/'+name,tofile='after/'+name))
(out/'audit-changes.patch').write_text(''.join(patch))
now=datetime.datetime.now(datetime.timezone.utc).isoformat()
shared_drift=[p for p,h in receipts['rust-final']['before'].items() if not (root/p).is_file() or sha(root/p)!=h]
comparison={'captured_utc':now,'shared_head':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'owned_files':{p:sha(root/p) for p in own},'changed_frozen_inputs_in_shared_checkout':shared_drift}
(out/'closeout-source-comparison.json').write_text(json.dumps(comparison,indent=2)+'\n')
# Preserve raw logs as text files so generic *.log ignores do not drop evidence.
dest.mkdir(parents=True,exist_ok=False)
artifacts={}
for path in sorted(out.rglob('*')):
 if not path.is_file() or path.name=='quanta-index-retrieval-bench':continue
 rel=path.relative_to(out)
 if path.suffix=='.log':rel=rel.with_suffix('.raw.txt')
 target=dest/rel;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(path,target)
 artifacts[str(rel)]=sha(target)
proof={
 'schema':'l5-definition-audit-v1','status':'VERIFIED','verified_scope':'L5 named-definition producer, source-bound preflight and selected consumer rejection tests',
 'captured_utc':now,'source':{'head':receipts['rust-final']['head'],'dirty':True,'snapshot':str(snapshot),'inputs_sha256':hashlib.sha256(json.dumps(receipts['rust-final']['before'],sort_keys=True,separators=(',',':')).encode()).hexdigest(),'owned_files':comparison['owned_files']},
 'shared_checkout_at_closeout':comparison,
 'findings':[
 {'id':'L5-DEF-01','priority':'P2','status':'VERIFIED','defect':'Go aliases were omitted; local types lost function and receiver ownership.','regression':'go_aliases_and_local_types_keep_function_and_receiver_ownership'},
 {'id':'L5-DEF-02','priority':'P2','status':'VERIFIED','defect':'Named Go interface method contracts were omitted.','regression':'go_named_interfaces_emit_owned_method_contracts'},
 {'id':'L5-DEF-03','priority':'P2','status':'VERIFIED','defect':'Named JS/TS expressions were omitted and their descendants lost the explicit owner.','regression':'named_expressions_keep_their_names_without_naming_anonymous_bindings'},
 {'id':'L5-DEF-04','priority':'P2','status':'VERIFIED','defect':'TS function signatures, interface/type-alias methods, abstract methods and method overload signatures were omitted.','regression':'typescript_callable_declarations_keep_owners_and_overload_spans'},
 {'id':'L5-DEF-05','priority':'P2','status':'VERIFIED','defect':'TS named module/namespace and enum declarations were omitted.','regression':'typescript_named_modules_and_enums_are_definitions_too'}],
 'test_summary':summary,'vite':load(out/'vite/vite-receipt.json'),
 'receipts':{name:{'path':f'l5-proof/definition-audit-20260927/{name}.json','sha256':sha(dest/f'{name}.json'),'command':a['command']} for name,a in receipts.items()},
 'negative_and_superseded_runs':{'rust-red':'FAILED as expected: five new inventory tests; 18 controls passed; stable original source','rust-green':'FAILED: updated source still produced original integration behavior, consistent with stale artifact reuse; copied source mtimes predated the earlier library build. Not accepted as fixed-source proof.','rust-rebuilt':'NOT_RUN: interrupted own process while blocked on shared Cargo build directory; superseded by final source and checkout-specific target.','python-compat':'VERIFIED only for its earlier snapshot; superseded by python-final'},
 'verification_controls':['Final Cargo target is checkout-scoped with no CARGO_TARGET_DIR override.','Frozen copied inputs receive fresh mtimes.','Fresh Vite binary producer policy is independently recomputed from snapshot source and lockfile.','Registered Rust identities match all executed owner tests; no skipped or filtered Rust owner cases.'],
 'excluded_scope':{'whole_repository_ci':'NOT_RUN','fresh_daemon_sdk_queries':'NOT_RUN','clean_commit_release_deployment':'NOT_RUN','complete_language_semantic_symbol_table':'NOT_APPLICABLE','ranking_performance':'NOT_APPLICABLE'},
 'artifacts_root':'l5-proof/definition-audit-20260927','artifact_sha256':artifacts}
report=handoff/'L5_DEFINITION_AUDIT.json';report.write_text(json.dumps(proof,indent=2)+'\n')
digest=sha(report)
text=f'''# L5 named-definition code audit

Status: **VERIFIED for the bounded producer/consumer scope below**. Five additional
inventory/ownership regressions were reproduced on the original source and repaired.

Final source: `{proof['source']['head']}` plus dirty overlay, frozen at
`{snapshot}`. The shared checkout's three edited source/inventory files match this
snapshot at closeout. A concurrent commit included the initial fixes; this task
performed no commit, push or reset. Other shared-checkout drift, if any, is listed
in the machine report and is not covered by this snapshot's qualification.

- Rust: **138 passed** (87 library, 3 binary, 25 chunking, 23 parser); the exact
  **135** registered owner test identities match executed tests.
- Python preflight/receipt compatibility: **16 passed**, 323 deselected.
- Fresh Vite CLI: **256** files retained, **255 Complete**, one intentional
  ParseFailed; **{proof['vite']['symbol_count']}** symbols. Allow-incomplete exits 0 and
  strict exits 2 after emitting the same complete census. Source/grammar/lockfile
  policy commitments are independently recomputed by the Python consumer.
- Changed Rust files pass rustfmt; changed files pass whitespace checks.
- All final receipts have identical source manifests and unchanged before/after inputs.

| Finding | Fixed behavior |
| --- | --- |
| L5-DEF-01 / P2 | Go `type Alias = ...` is emitted as type_alias. Local types retain `Function.Local` and `Receiver.Method.Local` ownership. |
| L5-DEF-02 / P2 | Named Go interfaces and interface aliases emit their own method contracts, including local interfaces. Two existing tests now select the concrete receiver by qualified name rather than accepting another same-name method. |
| L5-DEF-03 / P2 | Explicitly named JS/TS class/function/generator expressions emit a definition and qualify their descendants. Anonymous expressions receive no invented binding name. |
| L5-DEF-04 / P2 | TS function declarations without bodies, interface/type-alias methods, abstract methods and overload signatures retain their own spans and owners. |
| L5-DEF-05 / P2 | TS enum and module/namespace definitions are emitted together with their members. |

The handwritten regression inventories assert exact names, kinds, source slices,
owner fields, multiplicity and Complete counts across Go, JS/JSX and TS/TSX.
The original-source run failed all five new tests with 18 controls passing. The TS
type-alias method case was added during the subsequent completeness review.

A first fixed-source run still returned the original integration behavior. Copied
input mtimes predated the earlier library build, consistent with stale artifact
reuse; the new regressions caught the mismatch. That failure
and the interrupted follow-up remain in the evidence. Final verification uses a
fresh checkout-scoped Cargo target, fresh copied mtimes, and a source-policy check
against the actual CLI binary. No failed, interrupted or stale run is promoted.

Commands, source/environment identities, raw outputs and digests:
[L5_DEFINITION_AUDIT.json](L5_DEFINITION_AUDIT.json), SHA-256 `{digest}`.
Raw logs are retained as `.raw.txt` files under
[l5-proof/definition-audit-20260927](l5-proof/definition-audit-20260927).

This proves the supported named-definition forms exercised here, not a complete
language semantic symbol table. Variable/field/enum-member inventories, computed
property resolution and semantic name normalization are outside this change.
Whole-repository CI, fresh daemon/SDK query execution and release/deployment are
**NOT_RUN** on this snapshot. Historical audit and process receipts remain bound
to their original sources; their counts are not combined with this proof.
'''
(handoff/'L5_DEFINITION_AUDIT.md').write_text(text)
p=handoff/'L5_HANDOFF.md';s=p.read_text();marker='## Named-definition follow-up\n'
assert marker not in s
s=s.replace('# L5 — Parser and source-fact remediation\n','# L5 — Parser and source-fact remediation\n\nLatest audit: [L5_DEFINITION_AUDIT](L5_DEFINITION_AUDIT.md) records five further\nproducer inventory/ownership fixes, Rust 138, Python 16 and a fresh Vite census\nat `681dc3f0` plus its frozen overlay. Earlier receipts below keep their own scope.\n',1);p.write_text(s)
p=handoff/'L5_HANDOFF.source.json';a=load(p);a['definition_audit']={'path':report.name,'sha256':digest,'head':proof['source']['head']};p.write_text(json.dumps(a,indent=2)+'\n')
p=handoff/'L5_TEST_MIGRATION.json';a=load(p);a['counts']['rust']=135;a['after_sha256']=sha(snapshot/'benchmarks/retrieval/proof-required-tests.json');a['definition_audit']=report.name;a['definition_added_tests']=[f['regression'] for f in proof['findings']];a['execution_status']='Definition audit: Rust 138 and Python compatibility 16 passed on frozen 681dc3f0 plus overlay; fresh Vite census 256 retained files. Fresh daemon/SDK and repository-wide CI NOT_RUN. Earlier receipts retain their original identities.';p.write_text(json.dumps(a,indent=2)+'\n')
p=root/'docs/plans/sep-27-code-search-remediation/rfcs/CS-PROD-01-parser-coverage-and-vite.md';s=p.read_text().replace('Latest code audit: [L5_FINAL_CODE_AUDIT](../handoffs/L5_FINAL_CODE_AUDIT.md).','Latest code audit: [L5_DEFINITION_AUDIT](../handoffs/L5_DEFINITION_AUDIT.md).');p.write_text(s)
print(json.dumps({'report':str(report),'sha256':digest,'rust':138,'python':16,'vite_symbols':proof['vite']['symbol_count'],'copied_artifacts':len(artifacts),'shared_drift':shared_drift},indent=2))
