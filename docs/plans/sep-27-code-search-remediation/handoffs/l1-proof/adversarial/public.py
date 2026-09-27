import hashlib,json,os,shutil,socket,subprocess,sys,tempfile,time
from pathlib import Path
BASE=Path('/tmp/quanta-l1-rca.h1y4pja7')
ROOT=Path('/Users/songmin/Documents/code-new/quanta-index')
label=sys.argv[1]
build_label='audit-public-build-1'
if label=='1':
    result=subprocess.run([sys.executable,str(BASE/'run-audit.py'),'public',build_label],cwd=ROOT)
    if result.returncode: sys.exit(result.returncode)
receipt=json.loads((BASE/(build_label+'.receipt.json')).read_text())
binary=next(a for a in receipt['artifacts'] if a['target']['name']=='quanta-index-searchd' and 'bin' in a['target']['kind'])
original_binary=binary['path']
frozen=BASE/'searchd-proof-bin'
if not frozen.exists(): shutil.copy2(original_binary,frozen)
assert hashlib.sha256(frozen.read_bytes()).hexdigest()==binary['sha256'], 'daemon artifact digest changed'
binary=dict(binary,original_path=original_binary,path=str(frozen))
state=Path(tempfile.mkdtemp(prefix='qi-l1-public-',dir='/tmp')).resolve()
env=dict(os.environ)
env.update(QUANTA_INDEX_EMBEDDER='hash-dev',QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS='8',QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES=str(16*1024*1024),QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS='128',QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES=str(256*1024*1024),QUANTA_INDEX_L1_TEST_STATE_ROOT=str(state))
log=BASE/('audit-public-daemon-'+label+'.log')
start=time.time()
with log.open('wb') as output:
    child=subprocess.Popen([binary['path'],'serve','--state-root',str(state)],cwd=ROOT,env=env,stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
    try:
        deadline=time.monotonic()+30
        while True:
            if child.poll() is not None: raise RuntimeError('daemon exited before readiness')
            ready=True
            for name in ['query','control','ingest']:
                with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:
                    try: client.connect(str(state/'search-plane'/(name+'.sock')))
                    except OSError: ready=False
            if ready: break
            if time.monotonic()>=deadline: raise TimeoutError('daemon sockets not ready in 30s')
            time.sleep(.02)
        result=subprocess.run([sys.executable,str(BASE/'run-audit.py'),'public','audit-public-sdk-'+label],cwd=ROOT,env=env)
    finally:
        child.terminate()
        try: child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill(); child.wait()
record={'binary':binary,'state_root':str(state),'argv':[binary['path'],'serve','--state-root',str(state)],'daemon_log':str(log),'daemon_log_sha256':hashlib.sha256(log.read_bytes()).hexdigest(),'daemon_env':{k:v for k,v in env.items() if k in ['QUANTA_INDEX_RESOURCE_WAIT_SECONDS','QUANTA_INDEX_EMBEDDER','QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS','QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES','QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS','QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES','QUANTA_INDEX_L1_TEST_STATE_ROOT']},'started_unix':start,'ended_unix':time.time(),'test_exit':result.returncode}
(BASE/('audit-public-process-'+label+'.json')).write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps(record),flush=True)
sys.exit(result.returncode)
