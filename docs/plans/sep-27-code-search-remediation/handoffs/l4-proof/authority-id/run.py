import datetime,hashlib,json,os,pathlib,re,subprocess,sys
folder=pathlib.Path(__file__).parent;source=json.loads((folder/'snapshot.json').read_text());root=pathlib.Path(source['root']);label=sys.argv[1];command=sys.argv[2:]
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def inputs():return {p:digest(root/p) for p in source['manifest']}
def external():return {p:digest(pathlib.Path(p)) for p in source['external_dependency_inputs']}
before=inputs();ext_before=external();env=os.environ.copy();overrides={'CARGO_BUILD_JOBS':'2','QUANTA_INDEX_RESOURCE_WAIT_SECONDS':'900'}
for k in ['CARGO_TARGET_DIR','QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR','QUANTA_INDEX_BUILD_LANE']:env.pop(k,None)
env.update(overrides);started=datetime.datetime.now(datetime.timezone.utc).isoformat();log=folder/(label+'.log')
print(json.dumps({'started':label,'command':command,'root':str(root)}),flush=True)
with log.open('w') as stream:r=subprocess.run(command,cwd=root,env=env,stdout=stream,stderr=subprocess.STDOUT)
after=inputs();ext_after=external();text=log.read_text();binaries=[{'path':p,'sha256':digest(pathlib.Path(p))} for p in re.findall(r'Running .*? \((/[^)]+)\)',text)]
record={'started':started,'ended':datetime.datetime.now(datetime.timezone.utc).isoformat(),'cwd':str(root),'command':command,'exit_code':r.returncode,'environment_overrides':overrides,'ambient_build_inputs':{k:env.get(k) for k in ['RUSTFLAGS','RUSTUP_TOOLCHAIN','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','CARGO_INCREMENTAL']},'inputs_before':before,'inputs_after':after,'external_inputs_before':ext_before,'external_inputs_after':ext_after,'inputs_unchanged':before==after and ext_before==ext_after,'source_sha256':hashlib.sha256(json.dumps(before,sort_keys=True,separators=(',',':')).encode()).hexdigest(),'binaries':binaries,'test_summaries':re.findall(r'test result:.*',text),'log':str(log),'log_sha256':digest(log)}
(folder/(label+'.json')).write_text(json.dumps(record,indent=2)+'\n');print(json.dumps({k:record[k] for k in ['command','exit_code','source_sha256','inputs_unchanged','test_summaries','log_sha256']}),flush=True);sys.exit(r.returncode or (2 if not record['inputs_unchanged'] else 0))
