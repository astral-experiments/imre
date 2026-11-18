"""Focused, digest-rebound controls for the prospective finite-resource descendant."""
import collections,copy,hashlib,json,pathlib,re,sys,unittest
sys.path.insert(0,str(pathlib.Path.cwd()/'benchmarks/compatibility'))
import scorecard,raw_control
from extensions.data_resource959_v1 import authority
from extensions.data_resource959_v1.identity_roles import HeldCapture
enc=lambda x:json.dumps(x,sort_keys=True,separators=(',',':')).encode();sha=lambda b:hashlib.sha256(b).hexdigest()
class Stage(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  scorecard.select_snapshot(authority.REVISION);cls.m=authority.stage(scorecard)[0];cls.entry=authority.entry_for(scorecard,'configparser');cls.b=authority.effective_binding(scorecard,cls.entry);cls.c=cls.b['cohorts'][cls.b['cohort']]
 def read(self,r):return authority.read(scorecard,self.entry,self.b['roles'][r])
 def obj(self,r):return json.loads(self.read(r))
 def adapt(self,over=None,b=None):
  over=over or {};b=b or self.b;d=lambda r:over[r] if r in over else self.read(r);o=lambda r:json.loads(d(r));f=lambda r:authority.artifact(scorecard,self.entry,b['roles'][r]);return raw_control.adapt_selected_finite(scorecard,b['family'],b,b['cohorts'][b['cohort']],b['source'],f,d,lambda r:d(r).decode(),o,self.entry)
 def fail(self,over,pattern='.',b=None):
  with self.assertRaisesRegex(ValueError,pattern):self.adapt(over,b)
 def trace_change(self,edit):
  original=self.read('O0_trace');t=edit(original.decode()).encode();self.assertNotEqual(t,original);x=self.obj('O0_meta');x['trace_sha256']=sha(t);return {'O0_trace':t,'O0_meta':enc(x),'O0_withdraw_capture':enc(x)}
 def line_change(self,predicate,edit):
  def change(t):
   lines=t.splitlines();i=next(i for i,l in enumerate(lines) if predicate(l));lines[i]=edit(lines[i]);return '\n'.join(lines)+'\n'
  return self.trace_change(change)
 def policy_change(self,change):
  p=self.obj('policy');change(p);v=enc(p);out={'policy':v}
  for t in ('O0','O3'):
   x=self.obj(t+'_meta');x['policy_sha256']=sha(v);out[t+'_meta']=out[t+'_withdraw_capture']=enc(x)
  return out
 def test_real_rows_and_predecessor_receipts(self):
  records=scorecard.load_records('stdlib',scorecard.families());rows=[r for r in records if r['family']=='configparser'];self.assertEqual((len(rows),sum(r['paired'] for r in rows)),(344,339));self.assertEqual(collections.Counter(r['runs']['stock']['outcome'] for r in rows),{'pass':339,'skip':5});self.assertTrue(all(not r['paired'] and all(r['runs'][t]['outcome']=='skip' for t in scorecard.TIERS) for r in rows if r['runs']['stock']['outcome']=='skip'));self.assertEqual({r['family']:sum(x['paired'] for x in records if x['family']==r['family']) for r in records if r['family'] in {'fnmatch','json'}},{'fnmatch':24,'json':30});self.assertEqual(len(scorecard.load_records('script',scorecard.families(),scorecard.load_frozen_scripts())),4)
 def test_bounded_subset_math_and_zero_credit(self):
  bp=scorecard.ROOT/self.entry['binding'];tp=scorecard.ROOT/self.entry['staged_receipt'];saved=(bp.read_bytes(),tp.read_bytes());m=copy.deepcopy(self.m)
  try:
   b=copy.deepcopy(self.b);b['originals']=b['originals'][:10];b['observed_originals']=10;b['other_originals_not_run']=b['canonical_denominator']-10;bp.write_bytes(enc(b));tp.write_bytes(b'\n'.join(saved[1].splitlines()[:10])+b'\n');m['files'][self.entry['binding']]=sha(bp.read_bytes());m['files'][self.entry['staged_receipt']]=sha(tp.read_bytes());authority.verify_stage(scorecard,m)
   b['other_originals_not_run']=0;bp.write_bytes(enc(b));m['files'][self.entry['binding']]=sha(bp.read_bytes())
   with self.assertRaisesRegex(ValueError,'denominator'):authority.verify_stage(scorecard,m)
  finally:bp.write_bytes(saved[0]);tp.write_bytes(saved[1])
 def test_no_synthetic_read_proof_and_units(self):
  n=HeldCapture(scorecard,self.b,self.read,self.obj);p=n.identity_object('O0_build_json');self.assertIs(type(p['start_utc_ns']),int);self.assertIs(type(p['finish_utc_ns']),int);self.assertEqual(p['fd_before'],self.obj('O0_build_json')['held_fd_before']);self.assertTrue(all('path_stat' not in x and 'read_fstat_unchanged' not in x for x in p['fd_before'].values()));self.assertEqual(p['compiler_exit_code'],0)
 def test_rehashed_synthetic_proof_and_baseline(self):
  x=self.obj('O0_build_json')
  for key in ('held_fd_before','held_fd_after'):x[key]['cli'].update(path_stat={'ino':x[key]['cli']['ino']},read_fstat_unchanged=True)
  self.fail({'O0_build_json':enc(x)},'gate product|unsupported proof field');x=self.obj('capture_before');path=self.obj('O0_build_json')['target_argv'][0];x['inputs'][path]['ino']+=1;self.fail({'capture_before':enc(x)},'independent path witness')
 def test_rehashed_child_exit(self):
  x=self.obj('O0_build_json');t=self.read('O0_positive_trace').decode();pid=next(e[1] for e in raw_control.successful_exec_events(t.splitlines(),scorecard.require) if e[2]==x['target_argv'][0]);t=re.sub(r'(?m)^'+pid+r'(\s+exit_group\()0(\))',pid+r'\g<1>1\2',t);x['trace_sha256']=sha(t.encode());self.fail({'O0_build_json':enc(x),'O0_positive_trace':t.encode()},'missing raw trace operation|child exit|compiler')
 def test_rehashed_pid_overlap(self):
  def edit(t):
   es=raw_control.exec_sequence(t.splitlines(),scorecard.require,4);outer,inner=es[0][1],es[2][1];return '\n'.join(inner+l[len(outer):] if l.startswith(outer+' ') else l for l in t.splitlines())+'\n'
  self.fail(self.trace_change(edit),'PID must differ|supervisor|wrapper')
 def test_rehashed_exact_tier_environment(self):
  x=self.obj('O0_meta');x['environment']={**x['environment'],'IMRE_PYTHON':self.obj('O0_build_json')['target_argv'][0]};self.fail({'O0_meta':enc(x),'O0_withdraw_capture':enc(x)},'declared child environment')
 def test_rehashed_resource_authority(self):
  for f in (lambda p:p['families']['configparser']['readonly_data_files'][0]['source'].__setitem__('sha256','0'*64),lambda p:p['families']['configparser']['readonly_data_files'][0].__setitem__('runtime','/tmp/substituted-resource')):
   with self.subTest(change=f):self.fail(self.policy_change(f),'resource hash')
  key=next(iter(self.b['resource_roles'].values()));self.fail({key:self.read(key)+b'!'},'resource hash')
 def test_rehashed_resource_open_write_dirfd(self):
  runtime=self.obj('policy')['families'][self.b['family']]['readonly_data_files'][0]['runtime'];cwd=self.obj('O0_meta')['cwd']
  cases=[lambda l:l.replace('cfgparser.1','cfgparser.extra'),lambda l:l.replace('O_RDONLY|O_CLOEXEC','O_WRONLY|O_CLOEXEC'),lambda l:l.replace('AT_FDCWD<'+cwd+'>','9</tmp>').replace('"'+runtime+'"','"cfgparser.1"')]
  for f in cases:
   with self.subTest(change=f):self.fail(self.line_change(lambda l:'openat(' in l and 'cfgparser.1' in l,f),'withdrawn|unauthorized|resource')
 def test_rehashed_chain_order_image_exec(self):
  self.fail(self.line_change(lambda l:'execve("/usr/bin/bwrap"' in l,lambda l:l.replace('/usr/bin/bwrap','/usr/bin/other')),'wrapper|supervisor')
  self.fail(self.line_change(lambda l:'execve(' in l and 'O0.exe"' in l and 'absent-exec' not in l,lambda l:l.replace('O0.exe','other.exe')),'image|supervisor')
  self.fail(self.line_change(lambda l:'bin/python3"' in l and 'statx(' in l,lambda l:l+'\n'+l.split()[0]+' execve("/bin/true", ["/bin/true"], []) = 0'),'exec')
 def test_rehashed_python_policy(self):
  for edit in (lambda l:l.replace('= -1 ENOENT (No such file or directory)','= 0'),lambda l:l+'\n'+l,lambda l:l.replace('/bin/python3"','/bin/python3.14"')):
   with self.subTest(change=edit):self.fail(self.line_change(lambda l:'bin/python3"' in l and 'statx(' in l,edit),'Python|CPython')
 def test_rehashed_extra_source_and_python_open(self):
  trace=self.read('O0_trace').decode();target=raw_control.exec_sequence(trace.splitlines(),scorecard.require,4)[-1][1];cwd=self.obj('O0_meta')['cwd'];source=self.obj('O0_build_json')['target_argv'][self.obj('O0_build_json')['target_argv'].index('--extension-path')+1]+'/'+self.b['source']['path'].removeprefix('Lib/')
  for path in (source,'/usr/lib/libpython3.14.so'):
   with self.subTest(path=path):self.fail(self.line_change(lambda l:'statx(' in l and 'bin/python3"' in l,lambda l:l+'\n'+target+' openat(AT_FDCWD<'+cwd+'>, "'+path+'", O_RDONLY|O_CLOEXEC) = 40<'+path+'>'),'withdrawn|unauthorized|Python|libpython')
 def test_rehashed_gate_control_substitution(self):
  x=self.obj('gate_O0_direct_meta');trace=self.read('gate_O0_imported_trace');x['trace_sha256']=sha(trace);self.fail({'gate_O0_direct_trace':trace,'gate_O0_direct_meta':enc(x)},'gate|compiler|record')
 def test_rehashed_temporary_and_restoration(self):
  self.fail(self.line_change(lambda l:'unlinkat(' in l and '@test_' in l,lambda l:l.replace('= 0','= -1 EACCES (Permission denied)')),'unlink|lifecycle');self.fail(self.line_change(lambda l:'write(' in l and '@test_' in l,lambda l:l.replace('= 130290','= 0')),'write');x=self.obj('O0_cleanup');x['restoration']['restored']=False;self.fail({'O0_cleanup':enc(x)},'restoration|build/image')
 def test_old_receipt_substitution(self):
  from extensions.successor959_v1 import authority as old
  p=authority.safe(scorecard,old.entry_for(scorecard,'json')['receipt']);v=p.read_bytes()
  try:
   p.write_bytes(v+b'\n')
   with self.assertRaisesRegex(ValueError,'receipt bytes changed'):authority.verify_stage(scorecard,self.m)
  finally:p.write_bytes(v)
if __name__=='__main__':unittest.main(verbosity=1)
