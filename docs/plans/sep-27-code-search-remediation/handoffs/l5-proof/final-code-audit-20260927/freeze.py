from pathlib import Path
import subprocess,hashlib,shutil,json,datetime,sys
src=Path.cwd();dst=Path(sys.argv[1]);out=Path('/private/tmp/quanta-index-l5-final-audit-20260927')
head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()
assert subprocess.check_output(['git','-C',str(dst),'rev-parse','HEAD'],text=True).strip()==head
names=set(subprocess.check_output(['git','diff','--name-only','-z','HEAD']).decode().split('\0'))|set(subprocess.check_output(['git','ls-files','--others','--exclude-standard','-z']).decode().split('\0'));names.discard('')
def digest(p):
 if p.is_symlink():return 'symlink:'+str(p.readlink())
 return hashlib.sha256(p.read_bytes()).hexdigest() if p.is_file() else None
before={p:digest(src/p) for p in sorted(names)}
for p in sorted(names):
 a,b=src/p,dst/p
 if b.exists() or b.is_symlink():b.unlink()
 if a.exists() or a.is_symlink():
  b.parent.mkdir(parents=True,exist_ok=True)
  if a.is_symlink():b.symlink_to(a.readlink())
  else:shutil.copy2(a,b)
after={p:digest(src/p) for p in sorted(names)}
copied={p:digest(dst/p) for p in sorted(names)}
assert before==after==copied,'source changed while freezing'
receipt={'head':head,'dirty':True,'source':str(src),'snapshot':str(dst),'captured_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'overlay':copied}
(out/sys.argv[2]).write_text(json.dumps(receipt,indent=2)+'\n')
print('frozen overlay files:',len(copied))
