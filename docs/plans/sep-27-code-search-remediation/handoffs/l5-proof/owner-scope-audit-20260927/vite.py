from pathlib import Path
import datetime,hashlib,json,shutil,subprocess,sys
OUT=Path('/private/tmp/quanta-index-l5-owner-scope-audit-20260927/vite')
OUT.mkdir(exist_ok=True)
SNAPSHOT=Path('/Users/songmin/.codex/worktrees/l5-owner-scope-audit/quanta-index')
CONSUMER=Path('/Users/songmin/.codex/worktrees/l5-owner-scope-audit/quanta-index')
sys.path.insert(0,str(CONSUMER))
from tools.benchmark.retrieval import symbol_coverage
source=Path('/Users/songmin/Library/Caches/quanta-index/target')/hashlib.sha256(str(SNAPSHOT).encode()).hexdigest()[:16]/'test-daemon-lane/debug/quanta-index-retrieval-bench'
exe=OUT/'quanta-index-retrieval-bench'
sha=lambda path:hashlib.sha256(path.read_bytes()).hexdigest()
before=sha(source);shutil.copy2(source,exe);assert before==sha(source)==sha(exe)
repo=Path('/private/tmp/qi-full-lexical-20260927/full-repos/vite')
manifest=Path('/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-external-check/manifests/vite.json')
manifest_sha=sha(manifest);corpus=json.loads(manifest.read_text())
assert manifest_sha=='87ad16fe0c5626ee8ff625ddccf1645558c0b019d9229c5adcf24ffd0cbf7705'
assert subprocess.check_output(['git','-C',str(repo),'rev-parse','HEAD'],text=True).strip()==corpus['repository_commit']
for file in corpus['files']:
 assert sha(repo/file['path'])==file['file_sha256'],file['path']
commands=[];reports=[]
for policy,expected in [('allow-incomplete',0),('require-complete',2)]:
 path=OUT/f'vite-{policy}-built.json'
 command=[str(exe),'preflight','--repo',str(repo),'--manifest',str(manifest),'--symbol-coverage',policy,'--out',str(path)]
 started=datetime.datetime.now(datetime.timezone.utc).isoformat()
 with (OUT/f'vite-{policy}-built.log').open('w') as log:
  result=subprocess.run(command,cwd=SNAPSHOT,stdout=log,stderr=subprocess.STDOUT,timeout=180)
 commands.append({'command':command,'cwd':str(SNAPSHOT),'started_utc':started,'exit_code':result.returncode,'expected_exit':expected,'output_sha256':sha(path),'log_sha256':sha(OUT/f'vite-{policy}-built.log')})
 assert result.returncode==expected,commands[-1]
 envelope=json.loads(path.read_text());report=envelope['preflight'];reports.append(report)
 assert envelope['repository_commit']==corpus['repository_commit']
 assert report['producer_policy_sha256']==symbol_coverage.policy_digest(report['policy'],SNAPSHOT)
 assert report['grammar_identity']==symbol_coverage.grammar_identity(SNAPSHOT)
 assert report['lockfile_sha256']==sha(SNAPSHOT/'Cargo.lock')
 assert [(r['path'],r['source_sha256']) for r in report['files']]==sorted((r['path'],r['file_sha256']) for r in corpus['files'])
 for row in report['files']:symbol_coverage.validate_report_row(row)
 assert len(report['files'])==report['admitted_files']==256
 failed=[r for r in report['files'] if r['coverage']['state']!='complete']
 assert len(failed)==report['incomplete_files']==1
 assert failed[0]['path']=='packages/vite/src/node/ssr/__tests__/fixtures/errors/syntax-error.ts'
 assert failed[0]['coverage']['state']=='parse_failed'
 assert (repo/failed[0]['path']).read_text().strip()=='invalid code'
assert reports[0]==reports[1]
assert sha(exe)==before and sha(manifest)==manifest_sha
receipt={'status':'VERIFIED','scope':'fresh CLI parser census and strict/allow admission; no daemon/query/metrics execution','snapshot':str(SNAPSHOT),'head':subprocess.check_output(['git','-C',str(SNAPSHOT),'rev-parse','HEAD'],text=True).strip(),'binary_sha256':before,'manifest_sha256':manifest_sha,'corpus_commit':corpus['repository_commit'],'admitted_files':256,'complete_files':255,'parse_failed_files':1,'symbol_count':sum(r['coverage'].get('symbol_count',0) for r in reports[0]['files']),'policy_sha256':reports[0]['producer_policy_sha256'],'commands':commands}
(OUT/'vite-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
