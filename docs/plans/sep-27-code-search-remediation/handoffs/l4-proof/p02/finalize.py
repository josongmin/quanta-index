import datetime,hashlib,json,pathlib,re,shutil,subprocess
folder=pathlib.Path('/tmp/quanta-index-l4-followup');root=pathlib.Path.cwd();out=root/'docs/plans/sep-27-code-search-remediation/handoffs/l4-proof/p02'
frozen=json.loads((folder/'frozen.json').read_text());labels=['native','lexical-clippy','standalone-clippy'];runs={}
for label in labels:
 p=folder/('frozen-'+label+'-receipt.json');r=json.loads(p.read_text())
 assert r['exit_code']==0 and r['bound_inputs_unchanged'],label
 assert r['bound_inputs_after']==frozen['manifest'],label+' wrong source'
 log=folder/('frozen-'+label+'.log')
 assert hashlib.sha256(log.read_bytes()).hexdigest()==r['log_sha256'],label+' log changed'
 for p in [p,log]:shutil.copy2(p,out/p.name)
 runs[label]=r
summaries=runs['native']['test_summaries']
counts=[tuple(map(int,re.search(r'(\d+) passed; (\d+) failed; (\d+) ignored',s).groups())) for s in summaries]
assert counts==[(168,0,0),(4,0,0),(80,0,0),(20,0,0)],counts
for name in ['frozen.json','run_frozen.py','environment.json','dependency-audit.json','python-unicode-golden.json','format-final.log','format-final-receipt.json','finalize.py']:
 shutil.copy2(folder/name,out/name)
changed=[p for p,h in frozen['manifest'].items() if not (root/p).is_file() or hashlib.sha256((root/p).read_bytes()).hexdigest()!=h]
current_head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
record={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'proof_scope':'L4 owner/native adapter on frozen dirty source; no public SDK/daemon/workspace/heap/RSS qualification.','head':frozen['head'],'current_head':current_head,'branch':subprocess.check_output(['git','branch','--show-current'],text=True).strip(),'dirty_at_closeout':subprocess.check_output(['git','status','--short'],text=True),'frozen_root':frozen['root'],'source_sha256':runs['native']['source_sha256'],'source_manifest':'frozen.json','native_status':'VERIFIED','current_source_status':'VERIFIED' if not changed and current_head==frozen['head'] else 'BLOCKED','current_changed_inputs':changed,'tests':{'lexical_unit':168,'native_adapter':4,'regex_unit':80,'normalizer_unit':20,'passed':272,'failed':0,'ignored':0},'commands':{label:{'argv':run['command'],'env':run['environment_overrides'],'receipt':'frozen-'+label+'-receipt.json','log':'frozen-'+label+'.log','status':'VERIFIED'} for label,run in runs.items()},'audit':{'iterations':3,'fixed_p2':['Unobserved regex capture-state amplification','Normalization reservation omitted actual buffer layouts, spare capacity and stable-sort scratch'],'known_remaining_reproduced_p0_p2':[],'scope':'L4 source and directly adjacent shared consumers; not an exhaustive repository-wide defect census.'},'qualification_gaps':{'aggregate_regex_heap_bound':'BLOCKED','public_sdk_and_daemon':'NOT_RUN','workspace_and_release':'NOT_RUN','latency_rss_ranking_quality':'NOT_RUN'},'history':'round1/round2 and p02-* shared-tree runs are historical diagnostics, not final proof. Capture RED lacks a full pre-run dependency freeze.','artifacts':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.is_file() and p.name not in ['receipt.json','artifact-hashes.json']}}
(out/'receipt.json').write_text(json.dumps(record,indent=2)+'\n')
print(json.dumps({'source_sha256':record['source_sha256'],'current_source_status':record['current_source_status'],'changed':changed,'tests':record['tests'],'receipt_sha256':hashlib.sha256((out/'receipt.json').read_bytes()).hexdigest()},indent=2))
