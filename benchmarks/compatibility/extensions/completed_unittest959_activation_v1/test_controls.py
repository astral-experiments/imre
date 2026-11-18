"""Prospective finite identity, completion and binding controls over the sealed captures."""
import copy,functools,json,pathlib,sys,unittest
HERE=pathlib.Path(__file__).resolve().parent;ROOT=HERE.parents[3];B=ROOT/'benchmarks/compatibility';sys.path.insert(0,str(B));import raw_control,scorecard
from extensions.successor959_v1 import authority as common
from extensions.completed_unittest959_activation_v1.identity_roles import DirectCapture

class Controls(unittest.TestCase):
 @classmethod
 def setUpClass(cls):
  scorecard.select_snapshot('95908fb0acadc2a67304a8080e584deaef395746');cls.binding=json.loads((HERE/'binding.json').read_bytes())
 @functools.lru_cache(maxsize=None)
 def data(self,r):
  loc=self.binding['roles'][r];arc,sep,member=loc.partition('!')
  return common.archive_members(scorecard,arc,json.dumps(self.binding['archives'][arc],sort_keys=True))[member] if sep else (ROOT/loc).read_bytes()
 def obj(self,r):return json.loads(self.data(r))
 def core(self):
  b=self.binding;p=DirectCapture(scorecard,b,self.data,self.obj);return raw_control.finite_core(scorecard,b['family'],b['source'],{},lambda r:ROOT/b['roles'][r],self.data,lambda r:self.data(r).decode(),p.object,p.context())
 def test_real_full_shared_core(self):
  run,inv,paths=self.core();self.assertEqual([len(inv[t]) for t in scorecard.TIERS],[276]*3);self.assertEqual([run['runs'][t]['exit_code'] for t in scorecard.TIERS],[0,1,1]);self.assertEqual(sum(all(inv[t][k]['outcome']=='pass' for t in scorecard.TIERS) for k in inv['stock']),273);self.assertFalse(any(r['outcome'] not in {'pass','skip','error'} for t in inv for r in inv[t].values()))
 def test_native_subtest_skip_is_not_creditable(self):
  raw=self.data('O0_stderr');lines=raw.splitlines(keepends=True);event=b'IMRE_UNITTEST {"event":"subtest_skip","id":"parent","reason":"native-only"}\n';mutated=b''.join(lines[:-1])+event+lines[-1];trace=self.data('O0_trace').decode();pid=raw_control.exec_sequence(trace.splitlines(),scorecard.require,2)[-1][1];canonical=[(r['id'],r['occurrence']) for r in self.binding['originals']]
  import re
  summary=next(x for x in trace.splitlines() if x.startswith(pid+' ') and 'write(2<' in x and 'summary' in x);pipe=re.search(r'write\(2(<pipe:\[[0-9]+\]>)',summary)[1];write=f'{pid} write(2{pipe}, {json.dumps(event.decode())}, {len(event)}) = {len(event)}';rebound=trace.replace(summary,write+'\n'+summary,1)
  self.assertEqual(raw_control.traced_output(rebound.splitlines(),pid,2,scorecard.require),mutated)
  with self.assertRaisesRegex(ValueError,'summary, outcomes'):raw_control.completed_unittest(mutated,1,rebound,pid,scorecard.require,stdout=b'',canonical=canonical)
 def test_fresh_image_controller_substitution_rejected(self):
  b=self.binding;p=DirectCapture(scorecard,b,self.data,self.obj);ctx=p.context();m=p.object('O0_meta');build=p.object('O0_build_json')
  def wrong(role):return self.data('O3_build_controller') if role=='O0_build_controller' else self.data(role)
  with self.assertRaisesRegex(ValueError,'positive raw compiler'):raw_control.finite_identity_build(scorecard,'O0',ctx,build,m,wrong,lambda r:wrong(r).decode())
if __name__=='__main__':unittest.main()
