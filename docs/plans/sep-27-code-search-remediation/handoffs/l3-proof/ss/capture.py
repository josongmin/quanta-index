import hashlib, json, os, pathlib, subprocess, sys
root = pathlib.Path('/Users/songmin/Documents/code-new/quanta-index')
paths = subprocess.check_output(['git', 'ls-files', '--cached', '--others', '--exclude-standard'], cwd=root).decode().splitlines()
selected = sorted(set(p for p in paths if p.startswith(('crates/', '.cargo/', 'scripts/', 'tools/ci/', 'tools/benchmark/', 'vendor/tantivy-sstable/')) or p in ('Cargo.toml','Cargo.lock','rust-toolchain.toml','Justfile')))
files={p:hashlib.sha256((root/p).read_bytes()).hexdigest() for p in selected if (root/p).is_file()}
external_git_sources = []
metadata_path = pathlib.Path('/private/tmp/qi-l3-native-evidence/rca-metadata-updated.json')
if metadata_path.is_file():
  metadata = json.loads(metadata_path.read_text())
  for package in metadata['packages']:
    if not (package['source'] or '').startswith('git+'):
      continue
    directory = pathlib.Path(package['manifest_path']).parent
    entries = subprocess.check_output(['git','ls-files','--cached','--others','--exclude-standard'],cwd=directory).decode().splitlines()
    external_git_sources.append({
      'name':package['name'], 'source':package['source'], 'directory':str(directory),
      'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=directory).decode().strip(),
      'dirty':subprocess.check_output(['git','status','--porcelain=v1'],cwd=directory).decode().splitlines(),
    })
    for entry in sorted(set(entries)):
      path = directory/entry
      if path.is_file():
        files['external-git/'+package['name']+'/'+entry] = hashlib.sha256(path.read_bytes()).hexdigest()
value={
  'external_git_sources': external_git_sources,
  'head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root).decode().strip(),
  'dirty':subprocess.check_output(['git','status','--porcelain=v1'],cwd=root).decode().splitlines(),
  'scope':'crates, .cargo, scripts, tools/ci, tools/benchmark, Cargo manifests/lock, toolchain, Justfile and resolved Git dependency bytes; excludes unrelated benchmark/docs dirt',
  'files':files,
  'source_sha256':hashlib.sha256(json.dumps(files,sort_keys=True,separators=(',',':')).encode()).hexdigest(),
  'rustc':subprocess.check_output(['rustc','-Vv'],cwd=root).decode(),
  'environment':{key:os.environ.get(key) for key in ['RUSTUP_TOOLCHAIN','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_TARGET_DIR','CARGO_BUILD_JOBS','QUANTA_INDEX_RESOURCE_ADMISSION','QUANTA_INDEX_RESOURCE_WAIT_SECONDS','QUANTA_INDEX_CACHE_ROOT','CC','CXX']},
  'platform':subprocess.check_output(['uname','-a'],cwd=root).decode().strip(),
}
pathlib.Path(sys.argv[1]).write_text(json.dumps(value,indent=2)+'\n')
print(value['head'], value['source_sha256'], len(files), 'files')
