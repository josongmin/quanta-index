import datetime, hashlib, json, os, pathlib, platform, re, shutil, subprocess, sys, tempfile, time
root=pathlib.Path.cwd()
label=sys.argv[1]; command=sys.argv[2:]
out=root/'docs/plans/sep-27-code-search-remediation/handoffs/l2-proof'
out.mkdir(parents=True,exist_ok=True)
def call(argv): return subprocess.check_output(argv,cwd=root,text=True).strip()
def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def snapshot():
    paths=subprocess.check_output(['git','ls-files','-z','--cached','--others','--exclude-standard'],cwd=root).decode().split('\0')
    inputs={}
    for name in sorted(set(paths)):
        p=root/name
        if not p.is_file() or name.startswith(('docs/','artifacts/','.git/')): continue
        if p.suffix not in ('.rs','.toml','.lock','.sh','.py','.c','.h','.json') and p.name not in ('Justfile','rust-toolchain'): continue
        inputs[name]=digest(p)
    return {'head':call(['git','rev-parse','HEAD']), 'dirty':call(['git','status','--short']), 'inputs':inputs}
before=snapshot()
start=time.time()
receipt={'command':command,'cwd':str(root),'started_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'source_before':before,'rustc':call(['rustc','-vV']),'platform':platform.platform(),'environment':{k:v for k,v in os.environ.items() if k in ('RUSTUP_TOOLCHAIN','CARGO_TARGET_DIR','CARGO_BUILD_JOBS','QUANTA_INDEX_BUILD_LANE','QUANTA_INDEX_RESOURCE_BUDGET','QUANTA_INDEX_RESOURCE_ADMISSION','RUSTC_WRAPPER','RUSTFLAGS')}}
(out/(label+'.started.json')).write_text(json.dumps(receipt,indent=2)+'\n')
print('START',label,' '.join(command),flush=True)
log=out/(label+'.log')
with log.open('w') as f: result=subprocess.run(command,cwd=root,stdout=f,stderr=subprocess.STDOUT)
after=snapshot()
changed=[p for p in sorted(before['inputs'].keys()|after['inputs'].keys()) if before['inputs'].get(p)!=after['inputs'].get(p)]
body=log.read_text(errors='replace')
receipt.update({'exit_code':result.returncode,'elapsed_seconds':time.time()-start,'source_after':after,'changed_inputs':changed,'log':str(log.relative_to(root)),'log_sha256':digest(log),'test_summaries':re.findall(r'test result: [^\n]+',body),'outcome':'FAILED' if result.returncode else ('BLOCKED_SOURCE_DRIFT' if changed or before['head']!=after['head'] else 'VERIFIED_COMMAND_SCOPE'),'binaries':{}})
receipt['retained_executables'] = {}
if result.returncode == 0:
    for line in body.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict) or event.get('reason') != 'compiler-artifact' or not event.get('executable'):
            continue
        executable = pathlib.Path(event['executable'])
        before_digest = digest(executable)
        retained = pathlib.Path(tempfile.mkdtemp(prefix='qi-l2-binary-', dir='/tmp')) / executable.name
        shutil.copy2(executable, retained)
        retained_digest = digest(retained)
        if retained_digest != before_digest or digest(executable) != before_digest:
            raise RuntimeError(f'executable changed during custody capture: {executable}')
        receipt['binaries'][str(executable)] = retained_digest
        receipt['retained_executables'][str(retained)] = {'sha256': retained_digest, 'built_path': str(executable), 'package_id': event['package_id'], 'target': event['target'], 'profile': event['profile']}
for name in re.findall(r'\((/[^\n]+/(?:deps/)?[^/\s]+)\)',body):
    p=pathlib.Path(name)
    if p.is_file(): receipt['binaries'][name]=digest(p)
(out/(label+'.json')).write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps({k:receipt[k] for k in ('exit_code','elapsed_seconds','changed_inputs','test_summaries','outcome')}),flush=True)
print('\n'.join(body.splitlines()[-18:]),flush=True)
sys.exit(result.returncode)
