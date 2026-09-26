import hashlib, json, os, platform, subprocess, sys
import tomli as tomllib
from pathlib import Path
ROOT=Path('/Users/songmin/Documents/code-new/quanta-index')
BASE=Path('/tmp/quanta-l1-audit.touy8xnn')
rail,out=sys.argv[1:]
if rail == 'audit_native': rail = 'native'
seed={'native':'quanta-index-lexical','native_controls':'quanta-index-lexical','dispatcher':'quanta-index-search-plane','core_plan':'quanta-index-core','core_service':'quanta-index-core','base_window':'quanta-index-contract-base','dispatcher_controls':'quanta-index-search-plane','lexical_clippy':'quanta-index-lexical','dispatcher_clippy':'quanta-index-search-plane','native_all':'quanta-index-lexical','dispatcher_all':'quanta-index-search-plane','core_controls':'quanta-index-core'}[rail]
# Refresh metadata on each snapshot so manifest/dependency drift cannot hide.
raw=subprocess.check_output(['./scripts/cargow','metadata','--locked','--format-version','1','--no-deps'],cwd=ROOT)
meta=json.loads(raw)
by_path={str(Path(p['manifest_path']).parent.resolve()):p for p in meta['packages']}
queue=[p for p in meta['packages'] if p['name']==seed]
assert len(queue)==1,seed
packages={}
while queue:
    package=queue.pop()
    if package['id'] in packages: continue
    packages[package['id']]=package
    for dependency in package['dependencies']:
        if dependency.get('path'):
            key=str(Path(dependency['path']).resolve())
            assert key in by_path,('unresolved path dependency',key)
            queue.append(by_path[key])
files=set()
for package in packages.values():
    root=Path(package['manifest_path']).parent
    files.add(Path(package['manifest_path']))
    for folder in ['src']:
        if (root/folder).exists():
            files.update(p for p in (root/folder).rglob('*') if p.is_file())
    if (root/'build.rs').exists(): files.add(root/'build.rs')
    for target in package['targets']:
        if 'custom-build' in target['kind'] or 'lib' in target['kind'] or 'proc-macro' in target['kind']:
            files.add(Path(target['src_path']))
if rail in ['native','lexical_clippy','native_all']: files.add(ROOT/'crates/quanta-index-lexical/tests/l1_query_domain_window.rs')
if rail in ['native_controls','lexical_clippy','native_all']:
    test_root=ROOT/'crates/quanta-index-lexical/tests'
    for name in ['tantivy_smoke','execution_budget','ranked_pages','planner_authority','explain_candidate','regex_literal_alternation','regex_cache_bounds','unicode_normalization_goldens','cancellation_inside_search']:
        files.add(test_root/(name+'.rs'))
    files.update(p for p in (test_root/'support').rglob('*') if p.is_file())
if rail=='dispatcher_clippy':
    files.update(p for p in (ROOT/'crates/quanta-index-search-plane/tests').rglob('*') if p.is_file())
for filename in ['Cargo.toml','Cargo.lock','rust-toolchain.toml','scripts/cargow','scripts/quanta-index-env.sh','tools/ci/resource_admission.py','tools/benchmark/producer_execution.py','tools/ci/timing/rust_profile_history.py']:
    path=ROOT/filename
    assert path.is_file(),filename
    files.add(path)
if (ROOT/'.cargo').is_dir(): files.update(p for p in (ROOT/'.cargo').rglob('*') if p.is_file())
# --no-deps metadata omits transitive registry replacements. Follow Cargo.lock
# conservatively (all dependency edges) from this rail's package to bind local
# replacements that can actually affect it. Unrelated vendor metadata is not
# a query-behavior input; manifests, build script and full src are bound.
workspace_toml=tomllib.loads((ROOT/'Cargo.toml').read_text())
lock=tomllib.loads((ROOT/'Cargo.lock').read_text())
by_name={}
for package in lock['package']:
    by_name.setdefault(package['name'],[]).append(package)
def locked_dependency(ref):
    parts=ref.split(' ',2)
    candidates=by_name[parts[0]]
    if len(parts)>1:
        candidates=[p for p in candidates if p['version']==parts[1]]
    if len(parts)>2:
        assert parts[2].startswith('(') and parts[2].endswith(')'),ref
        candidates=[p for p in candidates if p.get('source')==parts[2][1:-1]]
    assert len(candidates)==1,('ambiguous locked dependency',ref)
    return candidates[0]
queue=[locked_dependency(seed)]
reachable={}
while queue:
    package=queue.pop()
    key=(package['name'],package['version'],package.get('source'))
    if key in reachable: continue
    reachable[key]=package
    queue.extend(locked_dependency(ref) for ref in package.get('dependencies',[]))
local_patches={}
for registry,patches in workspace_toml.get('patch',{}).items():
    for name,patch in patches.items():
        if not isinstance(patch,dict) or not patch.get('path'): continue
        package_name=patch.get('package',name)
        if not any(p['name']==package_name and 'source' not in p for p in reachable.values()): continue
        patch_root=(ROOT/patch['path']).resolve()
        assert patch_root.is_dir(),patch_root
        local_patches[registry+':'+name]=str(patch_root.relative_to(ROOT))
        files.add(patch_root/'Cargo.toml')
        files.update(p for p in (patch_root/'src').rglob('*') if p.is_file())
        if (patch_root/'build.rs').is_file():files.add(patch_root/'build.rs')
sha=lambda data:hashlib.sha256(data).hexdigest()
entries={str(p.relative_to(ROOT)):sha(p.read_bytes()) for p in sorted(files)}
canonical=json.dumps(entries,sort_keys=True,separators=(',',':')).encode()
result={'rail':rail,'seed':seed,'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
'dirty':subprocess.check_output(['git','status','--short'],cwd=ROOT,text=True),
'path_packages':sorted(p['name'] for p in packages.values()),'local_registry_patches':local_patches,'reachable_lock_packages':sorted(p['name']+'@'+p['version'] for p in reachable.values()),'files':entries,'source_manifest_sha256':sha(canonical),
'workspace_metadata_sha256':sha(raw),'snapshot_script_sha256':sha(Path(__file__).read_bytes()),
'scope':'Conservative path dependency closure, including optional/dev path dependencies and full src trees; native selected integration test(s) and their support helpers, or dispatcher src/lib tests. Reachable local registry patch manifests, build scripts and src included; other registry dependencies bound by Cargo.lock. Other integration test files and unrelated package sources excluded.',
'exclusions':'No clean-commit/repository/public SDK qualification; generated binary identity must be added from the executed command result.',
'python':sys.version,'tomli':tomllib.__version__,'platform':platform.platform(), 'rustc_identity':subprocess.check_output(['rustc','-Vv'],cwd=ROOT,text=True), 'inherited_build_env':{key:os.environ.get(key) for key in ['CI','CARGO_BUILD_JOBS','CARGO_TARGET_DIR','QUANTA_INDEX_SCCACHE','QUANTA_INDEX_PRESERVE_CARGO_TARGET_DIR','RUSTUP_TOOLCHAIN','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_BUILD_TARGET','QUANTA_INDEX_BUILD_LANE','QUANTA_INDEX_CACHE_ROOT','QUANTA_INDEX_RESOURCE_ADMISSION','QUANTA_INDEX_RESOURCE_TIMEOUT_SECONDS','QUANTA_INDEX_RESOURCE_WAIT_SECONDS']}}
Path(out).write_text(json.dumps(result,indent=2,sort_keys=True)+'\n')
print(json.dumps({'rail':rail,'files':len(entries),'source_manifest_sha256':result['source_manifest_sha256'],'output':out}))
