from pathlib import Path
import datetime,hashlib,json,re,shutil,subprocess,xml.etree.ElementTree as ET
out=Path('/private/tmp/quanta-index-l5-owner-scope-audit-20260927')
root=Path('/Users/songmin/Documents/code-new/quanta-index')
snapshot=Path('/Users/songmin/.codex/worktrees/l5-owner-scope-audit/quanta-index')
handoff=root/'docs/plans/sep-27-code-search-remediation/handoffs'
dest=handoff/'l5-proof/owner-scope-audit-20260927'
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
load=lambda p:json.loads(p.read_text())
freeze=load(out/'final-freeze.json')
required=load(snapshot/'benchmarks/retrieval/proof-required-tests.json')
receipts={name:load(out/f'{name}.json') for name in ['rust-final','python-final','format-final','vite-final','build-final']}
for name,a in receipts.items():
 assert a['exit_code']==0 and a['changed_inputs']==[],name
 assert a['head']==freeze['head'],name
 assert a['before']==receipts['rust-final']['before'],name
 assert a['log_sha256']==sha(out/f'{name}.log'),name
current=None;rust=[];bins=[]
for line in (out/'rust-final.log').read_text().splitlines():
 if 'Running unittests src/lib.rs' in line:current='quanta_index_retrieval_bench'
 elif 'Running unittests src/main.rs' in line:current='binary'
 elif 'Running tests/chunking_contract.rs' in line:current='chunking_contract'
 elif 'Running tests/l5_parser_regressions.rs' in line:current='l5_parser_regressions'
 match=re.fullmatch(r'test (.+) \.\.\. ok',line)
 if match:
  assert current is not None
  if current=='binary':bins.append(match[1])
  else:rust.append('quanta-index-retrieval-bench::'+current+'$'+match[1])
assert sorted(rust)==required['rust'] and len(rust)==138 and len(bins)==3
cases=list(ET.parse(out/'python-final.junit.xml').iter('testcase'))
assert len(cases)==16 and all(not any(x.tag in ('error','failure','skipped') for x in c) for c in cases)
assert len(required['python'])==339 and len(required['sdk'])==20
summary={'rust':{'passed':141,'registered':138,'registered_identities':sorted(rust),'additional_binary_tests':bins},'python':{'passed':16,'identities':[c.attrib['classname']+'.'+c.attrib['name'] for c in cases]}}
(out/'test-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
owned=freeze['owned_overlay_sha256']
for path,want in owned.items():
 assert sha(snapshot/path)==want,path
 assert sha(root/path)==want,path
vite=load(out/'vite/vite-receipt.json')
assert vite['head']==freeze['head'] and vite['admitted_files']==256 and vite['complete_files']==255 and vite['parse_failed_files']==1
binary=Path('/Users/songmin/Library/Caches/quanta-index/target')/hashlib.sha256(str(snapshot).encode()).hexdigest()[:16]/'test-daemon-lane/debug/quanta-index-retrieval-bench'
assert sha(binary)==vite['binary_sha256']
now=datetime.datetime.now(datetime.timezone.utc).isoformat()
shared_drift=[p for p,h in receipts['rust-final']['before'].items() if not (root/p).is_file() or sha(root/p)!=h]
comparison={'captured_utc':now,'shared_head':subprocess.check_output(['git','-C',str(root),'rev-parse','HEAD'],text=True).strip(),'owned_file_sha256':{p:sha(root/p) for p in owned},'changed_frozen_inputs_in_shared_checkout':shared_drift}
(out/'closeout-source-comparison.json').write_text(json.dumps(comparison,indent=2)+'\n')
dest.mkdir(parents=True,exist_ok=False)
artifacts={}
for path in sorted(out.rglob('*')):
 if not path.is_file() or path.name=='quanta-index-retrieval-bench':continue
 rel=path.relative_to(out)
 if path.suffix=='.log':rel=rel.with_suffix('.raw.txt')
 target=dest/rel;target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(path,target)
 artifacts[str(rel)]=sha(target)
proof={
 'schema':'l5-owner-scope-audit-v1','status':'VERIFIED','verified_scope':'L5 TypeScript definition ownership and dotted namespace names, plus prior owner regressions and selected Python consumer checks',
 'captured_utc':now,'source':{'head':freeze['head'],'dirty':True,'snapshot':str(snapshot),'overlay_sha256':owned,'inputs_sha256':hashlib.sha256(json.dumps(receipts['rust-final']['before'],sort_keys=True,separators=(',',':')).encode()).hexdigest()},
 'shared_checkout_at_closeout':comparison,
 'findings':[
  {'id':'L5-SCOPE-01','priority':'P2','status':'VERIFIED','defect':'Unscoped TS method_signature query emitted anonymous nested/parameter/field type members as methods owned by the nearest alias/function/class.','fix':'Accept only direct named interface/class members or object and intersection/parenthesized constituents of the alias value; skip unrelated anonymous positions before the symbol budget check.','regressions':['typescript_anonymous_type_members_do_not_become_false_owner_methods','typescript_alias_composition_keeps_only_owned_method_signatures']},
  {'id':'L5-SCOPE-02','priority':'P2','status':'VERIFIED','defect':'Dotted TS namespace names included source whitespace/comments and used the entire chain as local_name.','fix':'Read identifier/property AST segments and bind local, qualified and container names without formatting trivia.','regression':'dotted_namespaces_use_ast_segments_not_raw_spacing_or_comments'}],
 'test_summary':summary,'vite':vite,
 'receipts':{name:{'path':f'l5-proof/owner-scope-audit-20260927/{name}.json','sha256':sha(dest/f'{name}.json'),'command':a['command'],'input_manifest_sha256':hashlib.sha256(json.dumps(a['before'],sort_keys=True,separators=(',',':')).encode()).hexdigest()} for name,a in receipts.items()},
 'red_observations':{'anonymous_type':'FAILED: emitted Nested.run and use.run; direct positive Direct.run remained. The terminal output is in the task transcript, not an archived raw receipt.','dotted_namespace':'FAILED: emitted raw Outer . Inner and A/* gap */. B . C names. Terminal output is in the task transcript, not an archived raw receipt.','alias_composition':'NOT_RUN: queued focused command was interrupted before admission; the fixed-source test is covered by rust-final.'},
 'noncode_retry':{'status':'FAILED','cause':'First enhanced Vite rerun used a pre-existing --out path and was refused before parsing. The subsequent run uses fresh output paths and is the authoritative result.','receipt':'l5-proof/owner-scope-audit-20260927/vite-output-collision.json'},
 'excluded_scope':{'whole_repository_ci':'NOT_RUN','fresh_daemon_sdk_queries':'NOT_RUN','full_python_consumer_suite':'NOT_RUN','clean_commit_release_deployment':'NOT_RUN','ranking_performance':'NOT_APPLICABLE','all_language_semantic_symbol_table':'NOT_APPLICABLE'},
 'artifact_root':'l5-proof/owner-scope-audit-20260927','artifact_sha256':artifacts}
report=handoff/'L5_OWNER_SCOPE_AUDIT.json';report.write_text(json.dumps(proof,indent=2)+'\n');digest=sha(report)
md=f'''# L5 owner and namespace audit

Status: **VERIFIED for the bounded scope** on `{freeze['head']}` plus a three-file
L5 overlay at `{snapshot}`. Those three files match the shared checkout at closeout.
Other shared dirty inputs are not part of this snapshot's qualification.

Two further P2 defects were corrected:

- TS method signatures inside parameter, field and nested object types were
  incorrectly emitted as methods of a nearby named function, class or alias.
  Direct interface/class members and alias-owned direct, intersection and
  parenthesized object methods remain. Anonymous signatures are skipped before
  the symbol budget is charged.
- Dotted TS namespaces used raw whitespace/comments in qualified names and
  treated the full chain as a local name. The producer now derives each segment
  from AST identifiers, preserving source spans while canonicalizing identity.

Handwritten TS/TSX inventories assert exact name, kind, owner, source slice,
multiplicity and Complete count. Focused runs reproduced both false-owner and
namespace defects before repair. A focused alias-composition red run was queued
but interrupted before admission; its final regression passes in the full owner
run and is not presented as red proof.

- **Rust VERIFIED:** 141 passed (87 library, 3 binary, 25 chunking, 26 parser).
  All 138 registered owner identities match executed tests.
- **Python VERIFIED:** 16 selected preflight/receipt checks passed; 323 deselected.
- **Vite CLI VERIFIED:** 256 files retained, 255 Complete and one intentional
  ParseFailed; {vite['symbol_count']} symbols. Allow-incomplete exits 0; strict
  exits 2 with the same census. The consumer recomputed the binary producer
  policy, grammar identity, source hashes and manifest universe.
- **Format VERIFIED:** rustfmt and changed-file whitespace checks passed.

The exact commands, environment, source and raw output digests are in
[L5_OWNER_SCOPE_AUDIT.json](L5_OWNER_SCOPE_AUDIT.json), SHA-256 `{digest}`.
Raw logs are stored as `.raw.txt` under
[l5-proof/owner-scope-audit-20260927](l5-proof/owner-scope-audit-20260927).

Whole-repository CI, fresh daemon/SDK queries, the full Python consumer suite,
clean-source release and deployment are **NOT_RUN** on this snapshot. The earlier
L5 receipts keep their original source identities and are not combined with
these results. This audit does not claim a complete semantic symbol table for
all language forms.
'''
(handoff/'L5_OWNER_SCOPE_AUDIT.md').write_text(md)
p=handoff/'L5_HANDOFF.md';s=p.read_text();s=s.replace('# L5 — Parser and source-fact remediation\n','# L5 — Parser and source-fact remediation\n\nLatest audit: [L5_OWNER_SCOPE_AUDIT](L5_OWNER_SCOPE_AUDIT.md) records two\nadditional TS owner/name fixes on frozen `21029662` plus its L5 overlay.\nEarlier receipts below retain their original source and scope.\n',1);s=s.replace('Latest audit: [L5_DEFINITION_AUDIT]','Previous audit: [L5_DEFINITION_AUDIT]',1);p.write_text(s)
p=handoff/'L5_HANDOFF.source.json';a=load(p);a['owner_scope_audit']={'path':report.name,'sha256':digest,'head':freeze['head']};p.write_text(json.dumps(a,indent=2)+'\n')
p=handoff/'L5_TEST_MIGRATION.json';a=load(p);a['counts']['rust']=138;a['after_sha256']=sha(snapshot/'benchmarks/retrieval/proof-required-tests.json');a['owner_scope_audit']=report.name;a['owner_scope_added_tests']=['typescript_anonymous_type_members_do_not_become_false_owner_methods','typescript_alias_composition_keeps_only_owned_method_signatures','dotted_namespaces_use_ast_segments_not_raw_spacing_or_comments'];a['execution_status']='Owner-scope audit: Rust 141 and Python 16 passed on frozen 21029662 plus L5 overlay; fresh Vite census 256 files. Fresh daemon/SDK, full Python consumers and whole-repository CI NOT_RUN.';p.write_text(json.dumps(a,indent=2)+'\n')
p=root/'docs/plans/sep-27-code-search-remediation/rfcs/CS-PROD-01-parser-coverage-and-vite.md';s=p.read_text().replace('Latest code audit: [L5_DEFINITION_AUDIT](../handoffs/L5_DEFINITION_AUDIT.md).','Latest code audit: [L5_OWNER_SCOPE_AUDIT](../handoffs/L5_OWNER_SCOPE_AUDIT.md).');p.write_text(s)
print(json.dumps({'report':str(report),'sha256':digest,'rust_passed':141,'python_passed':16,'vite_symbols':vite['symbol_count'],'artifacts':len(artifacts),'shared_drift':shared_drift},indent=2))
