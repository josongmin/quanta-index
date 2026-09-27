import pathlib,json,hashlib,subprocess,difflib,datetime,shutil
p=pathlib.Path(__file__).parent;s=json.loads((p/'snapshot.json').read_text());r=pathlib.Path(s['root']);live=pathlib.Path(s['origin']);packages=json.loads((p/'selected-dependency-packages.json').read_text());prefixes=tuple('crates/'+n+'/' for n in packages)
def relevant(f):return f.startswith(prefixes) or f in ['Cargo.toml','Cargo.lock','rust-toolchain.toml','rustfmt.toml','Justfile'] or f.startswith(('.cargo/','scripts/'))
paths=set(subprocess.check_output(['git','ls-files','-c','-o','--exclude-standard','-z'],cwd=live).decode().split('\0'));paths={f for f in paths if f and relevant(f) and (live/f).is_file()};old={f for f in s['manifest'] if relevant(f)};delta={};patch=[]
for f in sorted(paths|old):
 a=(r/f).read_bytes() if f in old else b'';b=(live/f).read_bytes() if f in paths else b''
 if a==b:continue
 patch+=list(difflib.unified_diff(a.decode().splitlines(True),b.decode().splitlines(True),fromfile='a/'+f if f in old else '/dev/null',tofile='b/'+f if f in paths else '/dev/null'))
 delta[f]={'before':s['manifest'].get(f),'after':hashlib.sha256(b).hexdigest() if f in paths else None}
 if f in paths:(r/f).parent.mkdir(parents=True,exist_ok=True);(r/f).write_bytes(b);s['manifest'][f]=delta[f]['after']
 else:(r/f).unlink();s['manifest'].pop(f)
(p/'selected-refresh.patch').write_text(''.join(patch));record={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'live_head':subprocess.check_output(['git','rev-parse','HEAD'],cwd=live,text=True).strip(),'live_dirty':subprocess.check_output(['git','status','--porcelain=v1'],cwd=live,text=True),'changed_inputs':delta};(p/'selected-refresh.json').write_text(json.dumps(record,indent=2)+'\n');s['phase']='final-selected';s['source_sha256']=hashlib.sha256(json.dumps(s['manifest'],sort_keys=True,separators=(',',':')).encode()).hexdigest();(p/'snapshot.json').write_text(json.dumps(s,indent=2)+'\n');(p/'final-snapshot.json').write_text(json.dumps(s,indent=2)+'\n');print(json.dumps({'source_sha256':s['source_sha256'],'changed_inputs':delta},indent=2))
