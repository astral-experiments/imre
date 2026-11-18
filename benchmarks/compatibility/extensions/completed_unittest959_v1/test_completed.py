"""Prospective shared-core controls over authentic completed and partial raw captures."""
import hashlib,importlib.util,json,pathlib,shutil,subprocess,sys,tarfile,tempfile,unittest
HERE=pathlib.Path(__file__).resolve().parent;ROOT=HERE.parents[3];COMPAT=ROOT/'benchmarks/compatibility';sys.path.insert(0,str(COMPAT))
import scorecard
class Completed(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  cls.policy=json.loads((HERE/'policy.json').read_bytes());cls.tmp=tempfile.TemporaryDirectory(prefix='completed-unittest-');root=pathlib.Path(cls.tmp.name);rel=cls.policy['files'][0]['path'];p=root/rel;p.parent.mkdir(parents=True);p.write_bytes(subprocess.check_output(['git','show','--no-textconv',cls.policy['base_commit']+':'+rel],cwd=ROOT));assert subprocess.check_output(['git','rev-parse',cls.policy['base_commit']+':'+rel],cwd=ROOT,text=True).strip()==cls.policy['files'][0]['old_git_blob'];assert hashlib.sha256(p.read_bytes()).hexdigest()==cls.policy['files'][0]['old_sha256'];patch=ROOT/cls.policy['patch'];assert hashlib.sha256(patch.read_bytes()).hexdigest()==cls.policy['sha256'];subprocess.run(['git','apply',str(patch)],cwd=root,check=True);assert hashlib.sha256(p.read_bytes()).hexdigest()==cls.policy['files'][0]['post_sha256'];spec=importlib.util.spec_from_file_location('prospective_raw_control',p);cls.api=importlib.util.module_from_spec(spec);spec.loader.exec_module(cls.api)
  seal=json.loads((HERE/'evidence/COMPLETE.json').read_bytes());archive=HERE/'evidence/dataclasses-959.tar.gz';assert hashlib.sha256(archive.read_bytes()).hexdigest()==seal['archive_sha256']
  with tarfile.open(archive) as t:
   cls.members={x.name:t.extractfile(x).read() for x in t.getmembers() if x.isfile()}
  assert set(cls.members)==set(seal['files']) and all(len(v)==seal['files'][n]['bytes'] and hashlib.sha256(v).hexdigest()==seal['files'][n]['sha256'] for n,v in cls.members.items())
 @classmethod
 def tearDownClass(cls):cls.tmp.cleanup()
 def raw(self,t):return self.members['raw/'+t+'.stderr']
 def trace(self,t):return self.members['raw/'+t+'.trace'].decode()
 def pid(self,t):return self.api.exec_sequence(self.trace(t).splitlines(),scorecard.require,1 if t=='stock' else 2)[-1][1]
 def check(self,t,raw=None,trace=None,exit_code=None):return self.api.completed_unittest(self.raw(t) if raw is None else raw,(0 if t=='stock' else 1) if exit_code is None else exit_code,self.trace(t) if trace is None else trace,self.pid(t),scorecard.require)
 def test_real_completed_nonzero_and_stock(self):
  for t,p in (('stock',275),('O0',273),('O3',273)):
   with self.subTest(tier=t):index=self.check(t);self.assertEqual(len(index),276);self.assertEqual(sum(x['outcome']=='pass' for x in index.values()),p)
 def test_partial_even_with_rebound_stream_and_exit(self):
  raw=b'\n'.join(self.raw('O0').splitlines()[:-1])+b'\n';trace='\n'.join(x for x in self.trace('O0').splitlines() if not ('write(2<' in x and 'summary' in x))+'\n'
  with self.assertRaisesRegex(ValueError,'summary incomplete'):self.check('O0',raw,trace)
 def test_inconsistent_count_exit_and_truncation(self):
  with self.assertRaises(ValueError):self.check('O0',self.raw('O0').replace(b'"count": 276',b'"count": 275',1))
  with self.assertRaisesRegex(ValueError,'expected exit'):self.check('O0',exit_code=0)
  with self.assertRaisesRegex(ValueError,'traced child exit'):self.check('O0',trace=self.trace('O0').replace('exit_group(1)','exit_group(0)',1))
 def test_fabricated_pass_rebound_stderr_rejected(self):
  lines=self.raw('O0').decode().splitlines();found=[i for i,l in enumerate(lines) if '"event": "result"' in l];error=next(i for i in found if '"outcome": "error"' in lines[i]);good=next(i for i in found if '"outcome": "pass"' in lines[i]);prefix='IMRE_UNITTEST ';a=json.loads(lines[error][len(prefix):]);b=json.loads(lines[good][len(prefix):]);a.update(outcome='pass',details=[]);b.update(outcome='error',details=['invented']);lines[error]=prefix+json.dumps(a,ensure_ascii=True);lines[good]=prefix+json.dumps(b,ensure_ascii=True)
  with self.assertRaisesRegex(ValueError,'traced target writes'):self.check('O0',('\n'.join(lines)+'\n').encode())
 def test_missing_imported_control_rejected(self):
  meta=json.loads(self.members['raw/gate.O0.imported.json']);target=meta['target_argv'];cmd=target[3:];probe=target[0];poison=str(pathlib.Path(cmd[2]).parent/'dataclasses_gate_poison.py');trace=self.members['raw/gate.O0.imported.trace'].decode();diag=self.members['raw/gate.O0.imported.stderr'].decode();self.api.trace_gate(trace,cmd,probe,poison,diag,scorecard.require,expected_exit=1);bad='\n'.join(l for l in trace.splitlines() if not (('open(' in l or 'openat(' in l or 'openat2(' in l) and poison in l))+'\n'
  with self.assertRaisesRegex(ValueError,'imported poison'):self.api.trace_gate(bad,cmd,probe,poison,diag,scorecard.require,expected_exit=1)
 def test_nonzero_withdrawal_and_exact_failed_python_probe(self):
  t='O0';trace=self.trace(t);meta=json.loads(self.members['raw/O0.json']);cmd=meta['target_argv'];tree,stash,leaf,image=cmd[2:];cwd=meta['cwd'];positive=str(pathlib.Path(tree).parent.parent.parent/'launch/launcher.py');archive=str(pathlib.Path(positive).parent/'stock.source.py');probe={'pid':'target','operation':'statx','path':'/workspace/scratch/imre-95908-transferred-config-v1/product/imre-toolchain/bin/python3','errno':'ENOENT','count':1};self.api.trace_withdraw(scorecard,trace,image,cmd[0],tree,stash,leaf,cwd,scorecard.require,(positive,archive),True,expected_exit=1,python_probe=probe)
  line=next(l for l in trace.splitlines() if 'statx(' in l and 'bin/python3"' in l)
  for altered in (trace.replace(line,line+'\n'+line),trace.replace(line,line.replace('= -1 ENOENT (No such file or directory)','= 0'))):
   with self.assertRaisesRegex(ValueError,'Python|CPython'):self.api.trace_withdraw(scorecard,altered,image,cmd[0],tree,stash,leaf,cwd,scorecard.require,(positive,archive),True,expected_exit=1,python_probe=probe)
if __name__=='__main__':unittest.main()
