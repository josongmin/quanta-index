import hashlib,json,os,subprocess,sys,time
from pathlib import Path
ROOT=Path('/Users/songmin/Documents/code-new/quanta-index')
BASE=Path('/tmp/quanta-l1-green.diz44g')
rail,label=sys.argv[1:]
commands={
'native':['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-lexical','--test','l1_query_domain_window','--message-format=json'],
'dispatcher':['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-search-plane','--lib','--message-format=json','l1_']}
commands['native_controls']=['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-lexical','--message-format=json']
for test in ['tantivy_smoke','execution_budget','ranked_pages','planner_authority','explain_candidate','regex_literal_alternation','regex_cache_bounds','unicode_normalization_goldens','cancellation_inside_search']:
    commands['native_controls'].extend(['--test',test])
commands['core_plan']=['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-core','--lib','--message-format=json','domains::lexical::query_plan::']
commands['core_service']=['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-core','--lib','--message-format=json','domains::lexical::service::tests::']
commands['base_window']=['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-contract-base','--lib','--message-format=json','l1_logical_empty_tests']
commands['dispatcher_controls']=['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-search-plane','--lib','--message-format=json','--','query_dispatcher::tests::lexical::','query_dispatcher::tests::lexical_pages::','query_dispatcher::tests::structural::']
commands['lexical_clippy']=['./scripts/cargow','clippy','--locked','-j','2','-p','quanta-index-lexical','--no-deps','--message-format=json','--test','l1_query_domain_window']
for test in ['tantivy_smoke','execution_budget','ranked_pages','planner_authority','explain_candidate','regex_literal_alternation','regex_cache_bounds','unicode_normalization_goldens','cancellation_inside_search']:
    commands['lexical_clippy'].extend(['--test',test])
commands['lexical_clippy'].extend(['--','-D','warnings'])
commands['dispatcher_clippy']=['./scripts/cargow','clippy','--locked','-j','2','-p','quanta-index-search-plane','--lib','--tests','--no-deps','--message-format=json','--','-D','warnings']
commands['native_all']=commands['native_controls']+['--test','l1_query_domain_window','--no-fail-fast']
commands['dispatcher_all']=['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-search-plane','--lib','--message-format=json','--','l1_','query_dispatcher::tests::lexical::','query_dispatcher::tests::lexical_pages::','query_dispatcher::tests::structural::']
commands['core_controls']=['./scripts/cargow','test','--locked','-j','2','-p','quanta-index-core','--lib','--message-format=json','--','domains::lexical::query_plan::','domains::lexical::service::tests::']
command=commands[rail]
subprocess.run([sys.executable,str(BASE/'snapshot.py'),rail,str(BASE/(label+'.before.json'))],check=True,cwd=ROOT)
print(json.dumps({'running':command,'log':str(BASE/(label+'.log'))}),flush=True)
start=time.time()
with (BASE/(label+'.log')).open('wb') as output:
    result=subprocess.run(command,cwd=ROOT,stdout=output,stderr=subprocess.STDOUT)
subprocess.run([sys.executable,str(BASE/'snapshot.py'),rail,str(BASE/(label+'.after.json'))],check=True,cwd=ROOT)
raw=(BASE/(label+'.log')).read_bytes()
artifacts=[]
for line in raw.decode(errors='replace').splitlines():
    try: item=json.loads(line)
    except json.JSONDecodeError: continue
    if item.get('reason')=='compiler-artifact' and item.get('executable'):
        p=Path(item['executable'])
        artifacts.append({'path':str(p),'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'target':item.get('target'),'features':item.get('features'),'profile':item.get('profile'),'package_id':item.get('package_id'),'fresh':item.get('fresh')})
before=json.loads((BASE/(label+'.before.json')).read_text())
after=json.loads((BASE/(label+'.after.json')).read_text())
keys=set(before['files'])|set(after['files'])
changed=[key for key in sorted(keys) if before['files'].get(key)!=after['files'].get(key)]
receipt={'command':command,'exit_code':result.returncode,'started_unix':start,'ended_unix':time.time(),'log':str(BASE/(label+'.log')),'log_sha256':hashlib.sha256(raw).hexdigest(),'before':str(BASE/(label+'.before.json')),'after':str(BASE/(label+'.after.json')),'head_changed':before['head']!=after['head'],'source_changed':changed,'artifacts':artifacts,'executed_result_lines':[line for line in raw.decode(errors='replace').splitlines() if line.startswith('test result:') or line.startswith('running ')],'classification':'UNCLASSIFIED: inspect execution count, outcome, inputs and source drift before promoting evidence'}
(BASE/(label+'.receipt.json')).write_text(json.dumps(receipt,indent=2,sort_keys=True)+'\n')
print(json.dumps({key:value for key,value in receipt.items() if key not in ['artifacts']}),flush=True)
sys.exit(result.returncode)
