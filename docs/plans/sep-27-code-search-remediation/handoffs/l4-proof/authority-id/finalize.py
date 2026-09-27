import datetime,hashlib,json,pathlib,re,shutil,subprocess,tarfile,tempfile
p=pathlib.Path(__file__).parent;s=json.loads((p/'final-snapshot.json').read_text());live=pathlib.Path(s['origin']);out=live/'docs/plans/sep-27-code-search-remediation/handoffs/l4-proof/authority-id'
def digest(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def dump(path,obj):path.write_text(json.dumps(obj,indent=2)+'\n')
for name in ['final-native','final-clippy','final-format']:
 r=json.loads((p/(name+'.json')).read_text());assert r['exit_code']==0 and r['inputs_unchanged'];assert r['inputs_before']==s['manifest'];assert r['external_inputs_before']==s['external_dependency_inputs'];assert r['log_sha256']==digest(p/(name+'.log'))
expected=json.loads((p/'expected-regressions.json').read_text());log=(p/'final-native.log').read_text();missing=[name for names in expected.values() for name in names if not re.search(r'test (?:\S+::)?'+re.escape(name)+r' \.\.\. ok\n',log)];assert not missing,missing
summaries=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored; \d+ measured; (\d+) filtered out;',log);assert len(summaries)==3;assert all(tuple(v[1:])==('0','0','0') for v in summaries);total=sum(int(v[0]) for v in summaries);assert total==len(re.findall(r'^test \S+ \.\.\. ok$',log,re.M))
restored=pathlib.Path(tempfile.mkdtemp(prefix='qi-l4-proof-reconstruct-'))
with tarfile.open(p/'red-source.tar.gz') as t:t.extractall(restored)
for name in ['fix.patch','selected-refresh.patch','lint-fix.patch']:
 with (p/name).open() as stream:subprocess.run(['patch','-p1'],cwd=restored,stdin=stream,capture_output=True,check=True)
assert all(digest(restored/f)==h for f,h in s['manifest'].items());dump(p/'reconstruction.json',{'status':'VERIFIED','red_archive_sha256':digest(p/'red-source.tar.gz'),'patches':{n:digest(p/n) for n in ['fix.patch','selected-refresh.patch','lint-fix.patch']},'reconstructed_source_sha256':s['source_sha256'],'files':len(s['manifest'])})
packages=json.loads((p/'selected-dependency-packages.json').read_text());prefixes=tuple('crates/'+n+'/' for n in packages)
def relevant(f):return f.startswith(prefixes) or f in ['Cargo.toml','Cargo.lock','rust-toolchain.toml','rustfmt.toml','Justfile'] or f.startswith(('.cargo/','scripts/'))
changes={f:{'frozen':h,'live':digest(live/f) if (live/f).is_file() else None} for f,h in s['manifest'].items() if not (live/f).is_file() or digest(live/f)!=h}
paths=subprocess.check_output(['git','ls-files','-c','-o','--exclude-standard','-z'],cwd=live).decode().split('\0');added=[f for f in paths if f and relevant(f) and (live/f).is_file() and f not in s['manifest']]
close={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=live,text=True).strip(),'dirty':subprocess.check_output(['git','status','--porcelain=v1'],cwd=live,text=True),'all_bound_live_differences':changes,'selected_live_differences':{f:v for f,v in changes.items() if relevant(f)},'selected_added_paths':added,'selected_inputs_match':not any(relevant(f) for f in changes) and not added,'touched_files':{f:digest(live/f) for f in ['crates/quanta-index-lexical/src/text_docs.rs','crates/quanta-index-lexical/src/searcher/candidates.rs','crates/quanta-index-lexical/src/budgeted_search.rs','crates/quanta-index-lexical/tests/l2_file_mutation.rs']}}
dump(p/'live-closeout.json',close)
dump(p/'executed-regressions.json',{'status':'VERIFIED','expected':expected,'missing':missing,'test_summary_counts':summaries,'total_passed':total})
for f in p.iterdir():
 if f.is_file():shutil.copyfile(f,out/f.name)
receipt={'scope':'L4 stored text-authority ID strict decoding; lexical owner, L4 preview and L2 file mutation native tests','outcome':'VERIFIED','final_source_sha256':s['source_sha256'],'source_origin_head':s['head'],'source_origin_dirty':s['dirty'],'live_head':close['head'],'selected_inputs_match_live':close['selected_inputs_match'],'runs':{name:json.loads((p/(name+'.json')).read_text())['test_summaries'] for name in ['red','green-focused','native','final-native','final-clippy','final-format']},'final_passed':total,'evidence':{str(f.relative_to(out)):digest(f) for f in sorted(out.rglob('*')) if f.is_file() and f.name!='receipt.json'},'excluded':['whole workspace CI','SDK or daemon fresh-process E2E','public API baseline gate','fuzz smoke','aggregate regex compiler/cache heap proof','performance or RSS qualification','release/deployment/activation']}
dump(out/'receipt.json',receipt);print(json.dumps({'final_passed':total,'source_sha256':s['source_sha256'],'receipt_sha256':digest(out/'receipt.json'),'selected_inputs_match_live':close['selected_inputs_match'],'selected_live_differences':list(close['selected_live_differences'])},indent=2))
