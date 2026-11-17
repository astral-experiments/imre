"""Semantic tamper controls for the digest-bound hashlib finite cohort."""
import copy,hashlib,json,pathlib,sys,tempfile,unittest
from unittest.mock import patch
HERE=pathlib.Path(__file__).resolve();R=next(p for p in HERE.parents if (p/'benchmarks/compatibility/scorecard.py').is_file()) if '/evidence/' in str(HERE) else pathlib.Path('/workspace/imre-hashlib-strict');sys.path.insert(0,str(R/'benchmarks/compatibility'));import scorecard
PIN='8571ca2dbecabf8d7863c369dc776611fd57d8ad';D=R/'benchmarks/compatibility/revisions'/PIN;E=D/'evidence/hashlib/raw-control';sha=lambda p:hashlib.sha256(p.read_bytes()).hexdigest();loc=lambda p:p.relative_to(R).as_posix()
class BindingTests(unittest.TestCase):
 def setUp(self):
  a=json.loads((D/'activation.json').read_text()); self.entry=copy.deepcopy(a['families'].get('hashlib') or json.loads((E/'proposed-activation.json').read_text())); self.binding=json.loads((R/self.entry['sources']['binding']).read_text());self.rows=[json.loads(s) for s in (R/self.entry['receipt']).read_text().splitlines()];self.addCleanup(patch.stopall);patch.object(scorecard,'DATA_ROOT',D).start();patch.object(scorecard,'COHORT',{**scorecard.CANONICAL_COHORT,'imre_revision':PIN}).start();patch.object(scorecard,'snapshot_activation',return_value={'families':{'hashlib':self.entry}}).start()
 def artifact(self,tmp,role,data):
  p=tmp/role;p.write_bytes(data);self.binding['cohorts']['finite-native']['artifacts'][role]=loc(p);self.entry['files'][loc(p)]=sha(p);return p
 def reseal(self,tmp):
  p=tmp/'binding.json';p.write_text(json.dumps(self.binding)+'\n');self.entry['sources']['binding']=loc(p);self.entry['files'][loc(p)]=sha(p)
 def validate(self,row=None):
  r=scorecard.validate_record(row or self.rows[0],'stdlib',scorecard.families());scorecard.verify_stdlib_evidence(r);return r
 def test_inventory_and_individual_pairs(self):
  self.assertEqual(len(self.rows),82);self.assertEqual(sum(r['paired'] for r in self.rows),67);self.assertTrue(all(r['omission_reason'] is None for r in self.rows));[self.validate(r) for r in self.rows]
 def test_rehashed_valid_cohort_remap_cannot_inherit_full_inventory(self):
  self.binding['cohorts']['remapped']=copy.deepcopy(self.binding['cohorts']['finite-native']);self.binding['originals'][0]['launcher_cohort']='remapped'
  with tempfile.TemporaryDirectory(dir=E) as t:
   self.reseal(pathlib.Path(t))
   with self.assertRaisesRegex(ValueError,'finite cohort assigned and ordered stock/native inventories differ'):self.validate()
 def test_rehashed_stock_launcher_mismatch(self):
  c=self.binding['cohorts']['finite-native'];raw=(R/c['artifacts']['stock_launcher']).read_bytes();run=json.loads((R/c['artifacts']['run']).read_text())
  with tempfile.TemporaryDirectory(dir=E) as t:
   t=pathlib.Path(t);p=self.artifact(t,'stock_launcher',raw+b'# tamper\n');run['runs']['stock']['fresh_stock_launcher_sha256']=sha(p);self.artifact(t,'run',(json.dumps(run)+'\n').encode());self.reseal(t)
   with self.assertRaisesRegex(ValueError,'stock launcher bytes differ from native'):self.validate()
 def test_rehashed_source_hash_in_capture(self):
  c=self.binding['cohorts']['finite-native'];capture=json.loads((R/c['artifacts']['stock_capture']).read_text());capture['source_archive_sha256']='0'*64
  with tempfile.TemporaryDirectory(dir=E) as t:
   t=pathlib.Path(t);self.artifact(t,'stock_capture',(json.dumps(capture)+'\n').encode());self.reseal(t)
   with self.assertRaisesRegex(ValueError,'fresh stock capture metadata or raw stdout differs'):self.validate()
 def test_rehashed_wrong_stock_source_archive(self):
  with tempfile.TemporaryDirectory(dir=E) as t:
   t=pathlib.Path(t);self.artifact(t,'stock_source',b'# incorrect original\n');self.reseal(t)
   with self.assertRaisesRegex(ValueError,'stock source archive'):self.validate()
if __name__=='__main__':unittest.main()
