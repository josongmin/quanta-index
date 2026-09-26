import datetime,hashlib,json,os,pathlib,re,subprocess,sys
folder=pathlib.Path('/tmp/quanta-index-l4-followup')
source=json.loads((folder/'frozen.json').read_text());root=pathlib.Path(source['root'])
commands=[
 ('native',['test','-p','quanta-index-lexical','-p','quanta-index-lq-text-normalizer','-p','quanta-index-lq-regex','--lib','--test','l4_match_anchored_preview','--locked'],'test-integration-lane'),
 ('lexical-clippy',['clippy','-p','quanta-index-lexical','--lib','--test','l4_match_anchored_preview','--locked','--no-deps'],'clippy-lane'),
 ('standalone-clippy',['clippy','-p','quanta-index-lq-text-normalizer','-p','quanta-index-lq-regex','--all-targets','--locked','--no-deps'],'clippy-lane')]
commands=[commands[2],commands[1],commands[0]]
def hashes():return {p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in source['manifest']}
for label,args,lane in commands:
 before=hashes(); started=datetime.datetime.now(datetime.timezone.utc).isoformat()
 env=os.environ.copy(); overrides={'CARGO_BUILD_JOBS':'2','QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR':'1','CARGO_TARGET_DIR':'/Users/songmin/Library/Caches/quanta-index/target/e385f4e6b4fe8e9b/'+lane,'QUANTA_INDEX_RESOURCE_WAIT_SECONDS':'900'};env.update(overrides)
 logpath=folder/('frozen-'+label+'.log')
 print(json.dumps({'started':label,'cwd':str(root),'utc':started}),flush=True)
 with logpath.open('w') as log: result=subprocess.run(['./scripts/cargow',*args],cwd=root,env=env,stdout=log,stderr=subprocess.STDOUT)
 text=logpath.read_text();after=hashes();binaries=[]
 for path in re.findall(r'Running .*? \((/[^)]+)\)',text):
  binaries.append({'path':path,'sha256':hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()})
 record={'command':['./scripts/cargow',*args],'environment_overrides':overrides,'cwd':str(root),'started_utc':started,'ended_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'exit_code':result.returncode,'bound_inputs_before':before,'bound_inputs_after':after,'bound_inputs_unchanged':before==after,'source_sha256':hashlib.sha256(json.dumps(before,sort_keys=True,separators=(',',':')).encode()).hexdigest(),'test_summaries':re.findall(r'test result:.*',text),'log':str(logpath),'log_sha256':hashlib.sha256(logpath.read_bytes()).hexdigest(),'binaries':binaries,'scope':'Frozen copy of shared dirty source, owner/native targets only; not qualification of later shared edits, public SDK, daemon, whole workspace, heap/RSS or performance.'}
 (folder/('frozen-'+label+'-receipt.json')).write_text(json.dumps(record,indent=2)+'\n')
 print(json.dumps({k:v for k,v in record.items() if k not in ['bound_inputs_before','bound_inputs_after','binaries','environment_overrides']}),flush=True)
 if result.returncode or before!=after:sys.exit(result.returncode or 2)
