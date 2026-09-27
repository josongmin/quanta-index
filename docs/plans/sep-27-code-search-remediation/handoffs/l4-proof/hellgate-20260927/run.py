import datetime,hashlib,json,os,pathlib,re,subprocess,sys
folder=pathlib.Path(__file__).parent;label=sys.argv[1];command=sys.argv[2:];receipt_path=folder/(label+'.json')
receipt_path.write_text(json.dumps({'status':'NOT_RUN','label':label,'command':command,'command_executed':False},indent=2)+'\n')
source=json.loads((folder/'snapshot.json').read_text());root=pathlib.Path(source['root'])
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def bound_digest(p):
 try:return digest(p)
 except OSError:return None
def inputs():return {p:bound_digest(root/p) for p in source['manifest']}
def external():return {p:bound_digest(pathlib.Path(p)) for p in source['external_dependency_inputs']}
before=inputs();ext_before=external()
if before!=source['manifest'] or ext_before!=source['external_dependency_sha256']:
 blocked={'status':'BLOCKED','label':label,'reason':'frozen snapshot inputs changed before execution','command':command,'command_executed':False,'exit_code':2,'inputs_before':before,'external_inputs_before':ext_before,'matches_frozen_snapshot':False}
 receipt_path.write_text(json.dumps(blocked,indent=2)+'\n')
 print(json.dumps({'blocked':label,'reason':blocked['reason']}),flush=True)
 sys.exit(2)
env=os.environ.copy();overrides={'CARGO_BUILD_JOBS':'2','QUANTA_INDEX_RESOURCE_WAIT_SECONDS':'1800'}
for k in ['CARGO_TARGET_DIR','QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR','QUANTA_INDEX_BUILD_LANE']:env.pop(k,None)
env.update(overrides);started=datetime.datetime.now(datetime.timezone.utc).isoformat();log=folder/(label+'.log')
print(json.dumps({'started':label,'command':command,'root':str(root)}),flush=True)
with log.open('w') as stream:r=subprocess.run(command,cwd=root,env=env,stdout=stream,stderr=subprocess.STDOUT)
after=inputs();ext_after=external();text=log.read_text();binaries=[{'path':p,'sha256':digest(pathlib.Path(p))} for p in re.findall(r'Running .*? \((/[^)]+)\)',text)]
record={'started':started,'ended':datetime.datetime.now(datetime.timezone.utc).isoformat(),'cwd':str(root),'command':command,'command_executed':True,'exit_code':r.returncode,'environment_overrides':overrides,'ambient_build_inputs':{k:env.get(k) for k in ['RUSTFLAGS','RUSTUP_TOOLCHAIN','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','CARGO_INCREMENTAL']},'inputs_before':before,'inputs_after':after,'external_inputs_before':ext_before,'external_inputs_after':ext_after,'inputs_unchanged':before==after and ext_before==ext_after,'matches_frozen_snapshot':after==source['manifest'] and ext_after==source['external_dependency_sha256'],'source_sha256':hashlib.sha256(json.dumps(before,sort_keys=True,separators=(',',':')).encode()).hexdigest(),'binaries':binaries,'test_summaries':re.findall(r'test result:.*',text),'log':str(log),'log_sha256':digest(log)}
record['status']='VERIFIED' if r.returncode==0 and record['inputs_unchanged'] and record['matches_frozen_snapshot'] else 'FAILED' if r.returncode else 'BLOCKED'
receipt_path.write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({k:record[k] for k in ['status','command','exit_code','source_sha256','inputs_unchanged','matches_frozen_snapshot','test_summaries','log_sha256']}),flush=True);sys.exit(r.returncode or (2 if not record['inputs_unchanged'] or not record['matches_frozen_snapshot'] else 0))
