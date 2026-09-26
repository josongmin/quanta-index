import datetime,hashlib,json,os,pathlib,subprocess,sys
root=pathlib.Path.cwd()
out=pathlib.Path('/private/tmp/quanta-index-l5-final-audit-20260927')
name=sys.argv[1]; command=sys.argv[2:]
def snapshot():
 names=subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard','-z'],text=True).split('\0')
 roots=('benchmarks/retrieval/','crates/','vendor/','tools/benchmark/','tools/ci/')
 fixed={'Cargo.toml','Cargo.lock','pyproject.toml','uv.lock','rust-toolchain.toml','.cargo/config.toml','scripts/cargow','scripts/quanta-index-env.sh','Justfile'}
 paths=[p for p in sorted(set(names)) if p and (p in fixed or p.startswith(roots)) and (root/p).is_file()]
 return {p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in paths}
before=snapshot(); started=datetime.datetime.now(datetime.timezone.utc).isoformat()
with (out/(name+'.log')).open('w') as log:
 result=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
after=snapshot()
receipt={'cwd':str(root),'command':command,'environment':{k:v for k,v in os.environ.items() if k in ['CARGO_TARGET_DIR','CARGO_NET_OFFLINE','CARGO_BUILD_JOBS','RUSTUP_TOOLCHAIN','RUSTFLAGS','QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR','QUANTA_INDEX_RESOURCE_WAIT_SECONDS','QUANTA_BENCH_SDK_EVIDENCE_DIR','QUANTA_INDEX_SEARCHD_BIN','NEXTEST_EXPERIMENTAL_LIBTEST_JSON']},'exit_code':result.returncode,'started_utc':started,'ended_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'head':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'before':before,'after':after,'changed_inputs':[p for p in set(before)|set(after) if before.get(p)!=after.get(p)],'log_sha256':hashlib.sha256((out/(name+'.log')).read_bytes()).hexdigest()}
(out/(name+'.json')).write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps({k:v for k,v in receipt.items() if k not in ['before','after']}))
print((out/(name+'.log')).read_text()[-10000:])
sys.exit(result.returncode)
