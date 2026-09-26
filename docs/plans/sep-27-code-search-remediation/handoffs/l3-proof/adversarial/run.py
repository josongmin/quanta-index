import subprocess,sys,pathlib,json,hashlib,os,re
base=pathlib.Path('/private/tmp/qi-l3-native-evidence');label=sys.argv[1];command=sys.argv[2:]
env=dict(os.environ,CARGO_BUILD_JOBS='2',QUANTA_INDEX_RESOURCE_ADMISSION='1')
subprocess.run([sys.executable,str(base/'capture.py'),str(base/f'{label}-before.json')],env=env,check=True)
log=base/f'{label}.log'
with log.open('wb') as stream:
 result=subprocess.run(command,env=env,stdout=stream,stderr=subprocess.STDOUT)
subprocess.run([sys.executable,str(base/'capture.py'),str(base/f'{label}-after.json')],env=env,check=True)
a=json.loads((base/f'{label}-before.json').read_text());b=json.loads((base/f'{label}-after.json').read_text())
changed=sorted(f for f in a['files'].keys()|b['files'].keys() if a['files'].get(f)!=b['files'].get(f))
raw=log.read_text();binaries={}
for name in re.findall(r'Running .*?\((/[^\n]+)\)',raw):
 p=pathlib.Path(name)
 if p.is_file(): binaries[name]=hashlib.sha256(p.read_bytes()).hexdigest()
receipt={'command':command,'exit_code':result.returncode,'changed_inputs':changed,'head_before':a['head'],'head_after':b['head'],'log_path':str(log),'log_sha256':hashlib.sha256(log.read_bytes()).hexdigest(),'binary_sha256':binaries}
(base/f'{label}-result.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt));print('\n'.join(raw.splitlines()[-70:]));sys.exit(result.returncode)
