"""Static, read-only ELF inspection of the preserved saved images; never executes them."""
import hashlib,json,os,pathlib,stat,subprocess
D=pathlib.Path(__file__).resolve().parent;images={'O0':'/workspace/scratch/configparser-959-identity-recapture-v1/O0.exe','O3':'/workspace/scratch/configparser-959-explore-v1/O3.exe'};expected={'O0':'ffcca0b30099ab9aa5280842be51ea9d775f7c12621f54ebbc3e1c56a0d1a167','O3':'88b29d7e246d6cd7cf6cc3928c3e9754d491ac9ca889f8cb556b5bd92d0efe40'}
def sha_fd(fd):
 h=hashlib.sha256();off=0
 while b:=os.pread(fd,1048576,off):h.update(b);off+=len(b)
 return h.hexdigest()
def st(x):return {'dev':x.st_dev,'ino':x.st_ino,'size':x.st_size,'mode':stat.S_IMODE(x.st_mode),'ctime_ns':x.st_ctime_ns,'mtime_ns':x.st_mtime_ns}
sha=lambda p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest();report={'schema':'imre.compatibility.static-elf.v1','credit':0,'controller_sha256':sha(__file__),'tool':'/usr/bin/readelf','tool_sha256':sha('/usr/bin/readelf'),'images':{}}
for tier,path in images.items():
 fd=os.open(path,os.O_RDONLY|os.O_CLOEXEC)
 try:
  before=st(os.fstat(fd));digest=sha_fd(fd);magic=os.pread(fd,4,0);assert digest==expected[tier] and magic==b'\x7fELF' and before==st(os.stat(path));argv=['/usr/bin/readelf','-d',path];env={'PATH':'/usr/bin'};p=subprocess.run(argv,env=env,capture_output=True,check=True);assert b'Dynamic section' in p.stdout and b'libpython' not in p.stdout.lower() and not p.stderr;out=D/(tier+'.stdout');out.write_bytes(p.stdout);after=st(os.fstat(fd));post=sha_fd(fd);assert before==after==st(os.stat(path)) and digest==post;report['images'][tier]={'path':path,'sha256_before':digest,'sha256_after':post,'fd_before':before,'fd_after':after,'magic_hex':magic.hex(),'argv':argv,'environment':env,'exit_code':p.returncode,'stdout_sha256':sha(out),'stderr_sha256':hashlib.sha256(p.stderr).hexdigest()}
 finally:os.close(fd)
(D/'capture.json').write_text(json.dumps(report,sort_keys=True,indent=2)+'\n')
