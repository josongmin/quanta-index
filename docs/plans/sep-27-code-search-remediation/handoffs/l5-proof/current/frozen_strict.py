import hashlib,json,pathlib,subprocess
base=pathlib.Path('/private/tmp/quanta-index-l5-completion');e=base/'frozen-vite-runtime'
command=[str(e/'quanta-index-retrieval-bench'),'preflight','--repo','/private/tmp/qi-full-lexical-20260927/full-repos/vite','--manifest','/Users/songmin/Documents/code-new/qi-rb-coverage-set-2026-09-25/frozen-external-check/manifests/vite.json','--symbol-coverage','require-complete','--out',str(e/'strict-preflight.json')]
r=subprocess.run(command,capture_output=True,timeout=180)
(e/'strict.stdout.log').write_bytes(r.stdout);(e/'strict.stderr.log').write_bytes(r.stderr)
assert r.returncode==2,(r.returncode,r.stderr)
a=json.loads((e/'symbol-preflight.json').read_bytes());b=json.loads((e/'strict-preflight.json').read_bytes())
assert a['preflight']==b['preflight'] and b['symbol_coverage_policy']=='require-complete'
assert b['preflight']['admitted_files']==256 and b['preflight']['incomplete_files']==1
assert b'1 incomplete files of 256' in r.stderr
v={'status':'VERIFIED','command':command,'expected_exit':2,'actual_exit':r.returncode,'all_256_rows_equal_to_allow_profile':True,'preflight_sha256':hashlib.sha256((e/'strict-preflight.json').read_bytes()).hexdigest()}
(e/'strict-receipt.json').write_text(json.dumps(v,indent=2)+'\n');print(json.dumps(v))
