from pathlib import Path
import json,re,hashlib,xml.etree.ElementTree as ET
out=Path('/private/tmp/quanta-index-l5-final-audit-20260927')
root=Path('/Users/songmin/.codex/worktrees/l5-final-consumer-audit/quanta-index')
authority=json.loads((root/'benchmarks/retrieval/proof-required-tests.json').read_text())
xml=ET.parse(out/'python-consumers-final.junit.xml')
cases=list(xml.iter('testcase'))
assert cases and all(not any(x.tag in ('error','failure','skipped') for x in c) for c in cases)
identities=[c.attrib['classname']+'.'+c.attrib['name'] for c in cases]
selected=sorted(s for s in identities if s.startswith('tools.ci.tests.test_retrieval_benchmark.'))
assert selected==authority['python']
raw=(out/'rust-current-head.log').read_text()
current=None; rust=[];bins=[]
for line in raw.splitlines():
 if 'Running unittests src/lib.rs' in line:current='quanta_index_retrieval_bench'
 elif 'Running unittests src/main.rs' in line:current='binary'
 elif 'Running tests/chunking_contract.rs' in line:current='chunking_contract'
 elif 'Running tests/l5_parser_regressions.rs' in line:current='l5_parser_regressions'
 matched=re.fullmatch(r'test (.+) \.\.\. ok',line)
 if matched:
  assert current is not None
  if current=='binary':bins.append(matched[1])
  else:rust.append('quanta-index-retrieval-bench::'+current+'$'+matched[1])
assert sorted(rust)==authority['rust']
summary={'python':{'passed':len(cases),'registered':len(selected),'identities':identities},'rust':{'passed':len(rust)+len(bins),'registered':len(rust),'registered_identities':sorted(rust),'additional_binary_tests':bins}}
for name in ['rust-current-head','python-consumers-final']:
 receipt=json.loads((out/(name+'.json')).read_text());assert receipt['exit_code']==0 and receipt['changed_inputs']==[]
(out/'test-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print({k:{field:v[field] for field in ('passed','registered')} for k,v in summary.items()})
