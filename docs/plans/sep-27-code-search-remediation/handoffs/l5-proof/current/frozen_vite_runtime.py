import hashlib,json,pathlib,shutil,subprocess,sys,time
ROOT=pathlib.Path.cwd();sys.path.insert(0,str(ROOT))
from tools.benchmark.retrieval import symbol_coverage as coverage,run as pairrun
out=pathlib.Path('/private/tmp/quanta-index-l5-completion/frozen-vite-runtime');out.mkdir()
binroot=pathlib.Path('/Users/songmin/Library/Caches/quanta-index/target/e385f4e6b4fe8e9b/test-daemon-lane/debug')
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
for name in ('quanta-index-searchd','quanta-index-retrieval-bench'): shutil.copy2(binroot/name,out/name)
runner=out/'quanta-index-retrieval-bench';daemon=out/'quanta-index-searchd'
repo=pathlib.Path('/private/tmp/qi-full-lexical-20260927/full-repos/vite')
manifest=pathlib.Path('/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-external-check/manifests/vite.json')
corpus=json.loads(manifest.read_bytes());files=sorted(corpus['files'],key=lambda r:r['path'])
canon=lambda v:json.dumps(v,sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()
bad='packages/vite/src/node/ssr/__tests__/fixtures/errors/syntax-error.ts';good='packages/vite/src/node/config.ts'
assert (repo/bad).read_text().strip()=='invalid code'
assert 'function defineConfig' in (repo/good).read_text()
queries=[('malformed_text',f'path:{bad} invalid'),('broad_symbol','defineConfig'),('complete_symbol',f'path:{good} defineConfig')]
pack={'schema_version':3,'suite_id':'l5-vite-capability','suite_commitment_sha256':hashlib.sha256(b'L5 capability probes, no ranking claim').hexdigest(),'repository_commit':corpus['repository_commit'],'tokenizer':'qi-regex-v1','tokenizer_budget_version':'qb-v1','routes':['lexical','symbol'],'file_universe':files,'file_universe_digest':hashlib.sha256(canon(files)).hexdigest(),'comparison_contract':{'top_k':10,'tokenizer':'qi-regex-v1','tokenizer_budget_version':'qb-v1','output_unit_policy':'rank_prefix','span_unit':'byte_span_with_line_projection_v1'},'tasks':[{'task_id':name,'query':q,'query_sha256':hashlib.sha256(q.encode()).hexdigest()} for name,q in queries]}
(out/'query-pack.json').write_bytes(canon(pack))
command=[str(runner),'run','--repo',str(repo),'--manifest',str(manifest),'--strategy','whole_file','--query-pack',str(out/'query-pack.json'),'--routes','lexical,symbol','--query-input-policy','native','--top-k','10','--state-root',str(out/'state'),'--searchd-bin',str(daemon),'--searchd-expected-sha256',sha(daemon),'--embedder','hash-dev','--repo-id','l5-vite','--revision-id',corpus['repository_commit'],'--generation','1','--runner-name','l5-vite-probe','--runner-revision','sha256:'+sha(runner),'--run-id','l5-vite-capability','--blinding','attested','--isolation-method','local-source-capability-probe','--access-block-log','not-isolated-no-quality-claim','--symbol-coverage','allow-incomplete','--out',str(out/'record.json'),'--metrics-out',str(out/'phase-metrics.json'),'--diagnostics-out',str(out/'diagnostic.json'),'--symbol-preflight-out',str(out/'symbol-preflight.json'),'--refusal-out',str(out/'refusal.json'),'--io-timeout-secs','120','--ready-timeout-secs','60']
(out/'command.json').write_text(json.dumps(command,indent=2)+'\n')
with (out/'runner.log').open('wb') as log:r=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,timeout=600)
assert r.returncode==0,(r.returncode,(out/'runner.log').read_text())
record=json.loads((out/'record.json').read_bytes());metrics=json.loads((out/'phase-metrics.json').read_bytes())
coverage.verify_artifact(metrics,out/'phase-metrics.json',corpus)
pairrun._validate_phase_metrics(metrics,'actual Vite')
rows={(r['route'],r['task_id']):r for r in record['results']}
text=rows['lexical','malformed_text'];assert text['status'] in ('success','capped'),text
assert any(c['path']==bad for c in text['candidates']),text
for task in ('malformed_text','broad_symbol'):
 row=rows['symbol',task];assert row['status']=='error' and row['error']['code']=='SYMBOL_COVERAGE_INCOMPLETE',row
row=rows['symbol','complete_symbol'];assert row['status'] in ('success','capped') and row['candidates'],row
assert all(c['path']==good for c in row['candidates']),row
receipt={'status':'VERIFIED','scope':'frozen Vite all-file producer and actual daemon text/symbol capability','excluded':['ranking quality','performance','embedding-free publication','whole repository qualification'],'command':command,'manifest_sha256':sha(manifest),'corpus_commit':corpus['repository_commit'],'file_count':metrics['file_count'],'symbol_count':metrics['symbol_count'],'incomplete_files':metrics['symbol_incomplete_files'],'queries':[{'route':r['route'],'task_id':r['task_id'],'status':r['status'],'error':r['error'],'candidate_paths':[c['path'] for c in r['candidates']]} for r in record['results']],'artifacts':{p.name:sha(p) for p in out.iterdir() if p.is_file()}}
(out/'receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))
