import json,os,pathlib,subprocess,sys
root=pathlib.Path.cwd();out=pathlib.Path('/private/tmp/quanta-index-l5-completion/sdk-final');out.mkdir()
binroot=pathlib.Path('/Users/songmin/Library/Caches/quanta-index/target/e385f4e6b4fe8e9b/test-daemon-lane/debug')
env=dict(os.environ,QUANTA_INDEX_RESOURCE_WAIT_SECONDS='900',QUANTA_BENCH_SDK_EVIDENCE_DIR=str(out),QUANTA_INDEX_SEARCHD_BIN=str(binroot/'quanta-index-searchd'),NEXTEST_EXPERIMENTAL_LIBTEST_JSON='1')
base=['./scripts/cargow','--lane','test-daemon-lane','nextest']
selector=['-p','quanta-index-retrieval-bench','--test','sdk_roundtrip','--all-features','--locked']
commands=[]
def execute(cmd,stdout,stderr):
 commands.append(cmd);(out/'commands.json').write_text(json.dumps(commands,indent=2)+'\n')
 with (out/stdout).open('wb') as o,(out/stderr).open('wb') as e:r=subprocess.run(cmd,stdout=o,stderr=e,env=env)
 if r.returncode:print((out/stderr).read_text()[-6000:]);sys.exit(r.returncode)
execute(base+['list','-p','quanta-index-retrieval-bench','--lib','--test','chunking_contract','--test','l5_parser_regressions','--all-features','--locked','--message-format','json'],'rust-inventory.json','rust-inventory.stderr')
execute([sys.executable,'tools/benchmark/retrieval/proof_inventory.py','--verify',str(out/'rust-inventory.json'),'--role','rust'],'rust-inventory-verify.log','rust-inventory-verify.stderr')
execute(base+['list',*selector,'--message-format','json'],'inventory.json','inventory.stderr')
execute([sys.executable,'tools/benchmark/retrieval/proof_inventory.py','--verify',str(out/'inventory.json'),'--role','sdk'],'inventory-verify.log','inventory-verify.stderr')
execute(base+['run',*selector,'--test-threads','1','--message-format','libtest-json-plus','--message-format-version','0.1'],'nextest.jsonl','nextest.stderr')
execute([sys.executable,'tools/benchmark/retrieval/sdk_proof.py','--record',str(out/'actual-runner-record.json'),'--nextest',str(out/'nextest.jsonl'),'--nextest-inventory',str(out/'inventory.json'),'--runner-bin',str(binroot/'quanta-index-retrieval-bench'),'--out',str(out/'sdk-results.json')],'proof.log','proof.stderr')
print((out/'sdk-results.json').read_text())
