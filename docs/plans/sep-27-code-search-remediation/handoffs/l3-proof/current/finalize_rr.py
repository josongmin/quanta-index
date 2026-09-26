import pathlib,json,subprocess,hashlib,gzip,datetime,shutil,re,sys
root=pathlib.Path.cwd();temp=pathlib.Path('/private/tmp/qi-l3-native-evidence')
handoff=root/'docs/plans/sep-27-code-search-remediation/handoffs';dest=handoff/'l3-proof/current'
sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
old=json.loads((handoff/'L3_HANDOFF.source.json').read_text())
if 'historical_runs' not in old:
 shutil.copyfile(handoff/'L3_HANDOFF.source.json',dest/'pre-rr-handoff.source.json')
roots=json.loads((temp/'lexical-path-dependency-closure.json').read_text())+['crates/quanta-index-ipc','crates/quanta-index-sdk','crates/quanta-index-searchctl','vendor/tantivy-sstable']
front=['Cargo.toml','Cargo.lock','rust-toolchain.toml','Justfile','tools/ci/resource_admission.py','tools/benchmark/producer_execution.py']
def relevant(path):
 return path in front or path.startswith(('.cargo/','scripts/')) or any(path.startswith(x+'/') for x in roots)
subprocess.run([sys.executable,str(temp/'capture.py'),str(temp/'rr-final-current.json')],check=True)
current=json.loads((temp/'rr-final-current.json').read_text());runs=[]
labels=['rr-exact-wiring','rr-exact-wiring-2','rr-decode','rr-decode-2','rr-owner-final','rr-owner-final-2']
labels += [p.name.removesuffix('-result.json') for p in sorted(temp.glob('rr-owner-final-[3-9]-result.json'))]
for label in labels:
 receipt=json.loads((temp/f'{label}-result.json').read_text());artifacts=[];snapshots={}
 for suffix in ['before','after']:
  src=temp/f'{label}-{suffix}.json'; target=dest/f'{label}-{suffix}.json.gz'
  target.write_bytes(gzip.compress(src.read_bytes(),mtime=0));snapshots[suffix]=json.loads(src.read_text())
  artifacts.append({'artifact':str(target.relative_to(root)),'sha256':sha(target),'uncompressed_sha256':sha(src)})
 for suffix in ['.log','-result.json']:
  src=temp/f'{label}{suffix}';target=dest/src.name;shutil.copyfile(src,target)
  artifacts.append({'artifact':str(target.relative_to(root)),'sha256':sha(target)})
 assert receipt['log_sha256']==sha(dest/f'{label}.log')
 raw=(temp/f'{label}.log').read_text()
 counts=[dict(zip(['passed','failed','ignored','measured','filtered'],map(int,match))) for match in re.findall(r'test result: \w+\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out',raw)]
 drift=[p for p in receipt['changed_inputs'] if relevant(p)]
 stale=[p for p in snapshots['after']['files'].keys()|current['files'].keys() if relevant(p) and snapshots['after']['files'].get(p)!=current['files'].get(p)]
 status='FAILED' if receipt['exit_code'] else ('BLOCKED' if drift or stale or current['head']!=receipt['head_after'] else 'VERIFIED')
 runs.append({'id':label,**receipt,'status':status,'counts':counts,'scope':'Selected owner and consumer targets only; mock transports are not installed searchd proof','relevant_input_drift':drift,'changes_since_run':stale,'source_sha256_before':snapshots['before']['source_sha256'],'source_sha256_after':snapshots['after']['source_sha256'],'artifacts':artifacts})
final=runs[-1]
assert final['status']=='VERIFIED', (final['id'],final['status'],final['relevant_input_drift'],final['changes_since_run'])
assert final['counts'] and all(x['failed']==0 for x in final['counts'])
owned=sorted(set(old.get('owned_files_after',{}))|{'crates/quanta-index-lexical/src/searcher/predicate_plan.rs','crates/quanta-index-lexical/src/searcher/manual_scan.rs','crates/quanta-index-lexical/src/ranked_page_tests.rs','crates/quanta-index-lexical/tests/l3_exact_source.rs','crates/quanta-index-sdk/src/namespace.rs','crates/quanta-index-sdk/src/tests.rs','crates/quanta-index-searchctl/tests/cli_smoke.rs'})
owned=[p for p in owned if (root/p).is_file()]
first_party=[p for p in owned if p.endswith('.rs')]
vendor_rs=['vendor/tantivy-sstable/src/'+x+'.rs' for x in ['block_reader','dictionary','delta','vint']]
checks=[]
for label,cmd in [('format',['rustfmt','--edition','2024','--config','skip_children=true','--check',*first_party]),('vendor-format',['rustfmt','--edition','2021','--config','skip_children=true','--check',*vendor_rs]),('diff',['git','diff','--check'])]:
 target=dest/f'rr-static-{label}.log'
 with target.open('wb') as output:r=subprocess.run(cmd,stdout=output,stderr=subprocess.STDOUT)
 checks.append({'command':cmd,'status':'VERIFIED' if r.returncode==0 else 'FAILED','exit_code':r.returncode,'artifact':str(target.relative_to(root)),'sha256':sha(target)})
assert all(c['status']=='VERIFIED' for c in checks), checks
provenance=json.loads((root/'vendor/tantivy-sstable/quanta-provenance.json').read_text())
for file,digest in {**provenance['upstream_files_sha256'],**provenance['modified_files_sha256'],**provenance['additional_files_sha256']}.items():
 assert sha(root/'vendor/tantivy-sstable'/file)==digest,file
for name in ['capture.py','run.py','finalize_rr.py','lexical-path-dependency-closure.json']:
 shutil.copyfile(temp/name,dest/name)
src=temp/'rr-final-current.json';target=dest/'rr-final-current.json.gz';target.write_bytes(gzip.compress(src.read_bytes(),mtime=0))
claims={'A_exact_name_adapter_execution':'VERIFIED','A_original_facts_and_legacy_controls':'VERIFIED','B_source_grouping_cursor_and_scope':'VERIFIED','C_native_collection_work_byte_inflight_admission':'VERIFIED','C_ranked_key_predecode_admission':'VERIFIED','consumer_fixture_migration':'VERIFIED','request_wide_preparation_manual_output_admission':'NOT_APPLICABLE: outside the native-collection L3 objective; not claimed','installed_searchd_public_sdk_product':'NOT_RUN','whole_repository_qualification':'NOT_RUN','ranking_quality_performance_RSS':'NOT_RUN'}
value={'captured_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'head':current['head'],'dirty':current['dirty'],'state':'VERIFIED','claim_scope':'L3 owner correctness and selected consumer fixtures on a dirty working source; not whole repository or installed-product qualification','communication':'No inter-task sending, reading, waiting or subagents','resource_admission':'Canonical cargow shared build-test.lock, QUANTA_INDEX_RESOURCE_ADMISSION=1 CARGO_BUILD_JOBS=2','owned_files_after':{p:sha(root/p) for p in owned},'additional_runtime_inputs':{p:sha(root/p) for p in ['Cargo.toml','Cargo.lock',*vendor_rs]},'static_checks':checks,'runs':runs,'historical_runs':old.get('historical_runs',old.get('runs',[])),'current_snapshot':{'artifact':str(target.relative_to(root)),'sha256':sha(target),'uncompressed_sha256':sha(src),'source_sha256':current['source_sha256']},'path_dependency_closure':roots,'requested_claims':claims,'exclusions':['Allocator metadata/realloc internals and process RSS','Query preparation/native caches/manual scan materialization/required output payloads','Generic Boolean pruning performance; TermQuery native WAND is covered','Unselected suites, daemon installation, deployment, ranking quality and holdout metrics'],'artifact_provenance_note':'Raw failure and drifted executions remain archived. Only the final selected run is current VERIFIED evidence; past pass counts are not summed into it.'}
(handoff/'L3_HANDOFF.source.json').write_text(json.dumps(value,indent=2)+'\n')
print(json.dumps({'run':final['id'],'counts':final['counts'],'source_sha256':current['source_sha256'],'receipt_sha256':sha(handoff/'L3_HANDOFF.source.json'),'static':[x['status'] for x in checks]}))
