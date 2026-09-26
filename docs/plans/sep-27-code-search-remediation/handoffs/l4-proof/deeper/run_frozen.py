import datetime, hashlib, json, os, pathlib, platform, re, subprocess, sys
folder=pathlib.Path('/tmp/quanta-index-l4-deeper/verified');source=json.loads((folder/'frozen.json').read_text());root=pathlib.Path(source['root'])
lock='/Users/songmin/Library/Caches/quanta-index/resource-admission/build-test.lock'
commands=[
 ('core',['./scripts/cargow','test','-p','quanta-index-core','--lib','--locked'],'test-lib-lane'),
 ('wire',['./scripts/cargow','test','-p','quanta-index-contract-base','-p','quanta-index-contract','--lib','--test','l4_preview_emission','--test','l4_preview_wire','--test','ipc_query_result_v2_contract','--locked'],'test-integration-lane'),
 ('native',['./scripts/cargow','test','-p','quanta-index-lexical','--lib','--test','l4_match_anchored_preview','--test','cancellation_inside_search','--test','execution_budget','--locked'],'test-integration-lane'),
 ('clippy',['./scripts/cargow','clippy','-p','quanta-index-contract-base','-p','quanta-index-contract','-p','quanta-index-lexical','-p','quanta-index-core','--lib','--test','l4_preview_emission','--test','l4_preview_wire','--test','l4_match_anchored_preview','--locked','--no-deps'],'clippy-lane'),
 ('public-api',['python3','tools/ci/resource_admission.py','--lock',lock,'--wait-seconds','900','--timeout-seconds','1800','--','just','rust-public-api'],'public-api-lane'),
 ('fuzz',['python3','tools/ci/resource_admission.py','--lock',lock,'--wait-seconds','900','--timeout-seconds','1800','--','just','rust-fuzz-smoke'],None),
]
if len(sys.argv)>1:commands=[next(c for c in commands if c[0]==label) for label in sys.argv[1:]]
for label,cmd,lane in commands:
 manifest=source['all_copied_files'] if label in ['public-api','fuzz'] else source['manifest']
 def hashes():return {p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in manifest}
 before=hashes();started=datetime.datetime.now(datetime.timezone.utc).isoformat();env=os.environ.copy()
 overrides={'CARGO_BUILD_JOBS':'2','QUANTA_INDEX_RESOURCE_WAIT_SECONDS':'900'}
 # Keep the canonical checkout-scoped target directory; never share snapshot artifacts.
 for key in ['CARGO_TARGET_DIR','QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR','QUANTA_INDEX_BUILD_LANE']:
  env.pop(key, None)
 env.update(overrides);logpath=folder/('final-'+label+'.log')
 print(json.dumps({'started':label,'cwd':str(root),'utc':started}),flush=True)
 with logpath.open('w') as log:result=subprocess.run(cmd,cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
 contents=logpath.read_text();after=hashes();binaries=[]
 for path in re.findall(r'Running .*? \((/[^)]+)\)',contents):
  p=pathlib.Path(path);binaries.append({'path':path,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()})
 expected=[]
 for path in {'wire':['crates/quanta-index-contract-base/tests/l4_preview_emission.rs','crates/quanta-index-contract/tests/l4_preview_wire.rs'],'native':['crates/quanta-index-lexical/tests/l4_match_anchored_preview.rs']}.get(label,[]):
  expected.extend(re.findall(r'#\[test\]\s*fn (\w+)',(root/path).read_text()))
 missing=[name for name in expected if not re.search(r'^test '+re.escape(name)+r' \.\.\. ok$',contents,re.M)]
 record={'expected_regressions':expected,'missing_executed_regressions':missing,'command':cmd,'environment_overrides':overrides,'platform':platform.platform(),'cwd':str(root),'started_utc':started,'ended_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'exit_code':result.returncode,'bound_inputs_before':before,'bound_inputs_after':after,'bound_inputs_unchanged':before==after,'source_sha256':hashlib.sha256(json.dumps(before,sort_keys=True,separators=(',',':')).encode()).hexdigest(),'test_summaries':re.findall(r'test result:.*',contents),'log':str(logpath),'log_sha256':hashlib.sha256(logpath.read_bytes()).hexdigest(),'binaries':binaries,'scope':'Frozen shared dirty source: selected owner/native and wire targets. This is not live shared-tree, SDK process, daemon, whole-workspace, release, heap/RSS or performance qualification.'}
 (folder/('final-'+label+'-receipt.json')).write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({k:record[k] for k in ['command','exit_code','bound_inputs_unchanged','source_sha256','test_summaries','log_sha256']}),flush=True)
 if missing or before!=after or (result.returncode and label not in ['public-api','fuzz']):sys.exit(result.returncode or 2)
