"""Exercise activated raw controls against the immutable merged snapshot."""
import copy
import hashlib
import json
import tempfile
import unittest
from collections import Counter
from pathlib import Path
from unittest.mock import patch

import scorecard

PIN = '8571ca2dbecabf8d7863c369dc776611fd57d8ad'
SNAPSHOT = scorecard.HERE / 'revisions' / PIN


def digest(path): return hashlib.sha256(path.read_bytes()).hexdigest()
def locator(path): return path.relative_to(scorecard.ROOT).as_posix()


class RawControlTests(unittest.TestCase):
    def setUp(self):
        activation = json.loads((SNAPSHOT / 'activation.json').read_text())
        self.entries = {f: copy.deepcopy(activation['families'][f]) for f in ('csv','base64')}
        self.addCleanup(patch.stopall)
        patch.object(scorecard, 'DATA_ROOT', SNAPSHOT).start()
        patch.object(scorecard, 'COHORT', {**scorecard.CANONICAL_COHORT, 'imre_revision': PIN}).start()
        patch.object(scorecard, 'snapshot_activation', return_value={'families': self.entries}).start()

    def rows(self): return scorecard.load_records('stdlib', scorecard.families())

    def test_real_receipts_and_five_child_execution_originals(self):
        rows = self.rows()
        self.assertEqual(len(rows),169)
        self.assertEqual(Counter((r['family'],r['paired']) for r in rows), {('csv',True):122,('csv',False):6,('base64',True):35,('base64',False):6})
        for family in ('csv','base64'):
            subset=[r for r in rows if r['family']==family]
            print(family, {t:dict(Counter(r['runs'][t]['outcome'] for r in subset)) for t in scorecard.TIERS}, 'validated pairs',sum(r['paired'] for r in subset))
        child=[r for r in rows if r['family']=='base64' and '.TestMain.' in r['id']]
        self.assertEqual(len(child),5)
        self.assertTrue(all(r['runs']['stock']['outcome']=='pass' and all(r['runs'][t]['outcome']=='not_run' for t in ('O0','O3')) and r['omission_reason'] is None and not r['paired'] for r in child))

    def test_rehashed_raw_openat2_witness_is_rejected(self):
        entry=self.entries['csv']; binding=json.loads((scorecard.ROOT/entry['sources']['binding']).read_text())
        with tempfile.TemporaryDirectory(dir=SNAPSHOT/'evidence/csv/raw-control') as tmp:
            tmp=Path(tmp); trace=(scorecard.ROOT/binding['cohorts']['finite-native']['artifacts']['O0_trace']).read_text(); lines=trace.splitlines(); child=next(line.split()[0] for line in lines if 'execve(' in line and 'csv-O0.exe", [' in line and 'withdraw' not in line)
            ix=next(i for i,line in enumerate(lines) if line.startswith(child+' exit_group('));lines.insert(ix,f'{child} openat2(AT_FDCWD, "test_csv.py", {{flags=O_RDONLY}}, 24) = 3')
            witness=tmp/'tampered.trace';witness.write_text('\n'.join(lines)+'\n');binding['cohorts']['finite-native']['artifacts']['O0_trace']=locator(witness);entry['files'][locator(witness)]=digest(witness)
            # Recompute the metadata and binding too; the claimed-good booleans cannot override the raw witness.
            run=json.loads((scorecard.ROOT/binding['cohorts']['finite-native']['artifacts']['run']).read_text());run['runs']['O0']['withdrawal']['no_source_access']=True;run['runs']['O0']['withdrawal']['no_child_exec']=True
            meta=tmp/'run.json';meta.write_text(json.dumps(run)+'\n');binding['cohorts']['finite-native']['artifacts']['run']=locator(meta);entry['files'][locator(meta)]=digest(meta)
            bound=tmp/'binding.json';bound.write_text(json.dumps(binding)+'\n');entry['sources']['binding']=locator(bound);entry['files'][locator(bound)]=digest(bound)
            with self.assertRaisesRegex(ValueError,'native opened CPython, libpython, withdrawn source or matching bytecode'):
                self.rows()

    def test_second_launcher_cannot_inherit_first_gate(self):
        entry = self.entries['csv']
        binding = json.loads((scorecard.ROOT / entry['sources']['binding']).read_text())
        binding['cohorts']['second-launcher'] = copy.deepcopy(binding['cohorts']['finite-native'])
        binding['originals'][0]['launcher_cohort'] = 'second-launcher'
        with tempfile.TemporaryDirectory(dir=SNAPSHOT/'evidence/csv/raw-control') as tmp:
            tmp = Path(tmp); bad = tmp/'bad-gate.stderr'; bad.write_text('expected str, got bool\n')
            binding['cohorts']['second-launcher']['artifacts']['gate_stderr'] = locator(bad)
            entry['files'][locator(bad)] = digest(bad)
            bound = tmp/'binding.json'; bound.write_text(json.dumps(binding)+'\n')
            entry['sources']['binding'] = locator(bound); entry['files'][locator(bound)] = digest(bound)
            with self.assertRaisesRegex(ValueError, 'missing imported int/bool diagnostic'):
                self.rows()

    def test_unmapped_original_cannot_inherit_family_control(self):
        entry = self.entries['csv']
        binding = json.loads((scorecard.ROOT / entry['sources']['binding']).read_text())
        binding['originals'][0]['launcher_cohort'] = 'unverified-launcher'
        with tempfile.TemporaryDirectory(dir=SNAPSHOT/'evidence/csv/raw-control') as tmp:
            bound = Path(tmp)/'binding.json'; bound.write_text(json.dumps(binding)+'\n')
            entry['sources']['binding'] = locator(bound); entry['files'][locator(bound)] = digest(bound)
            with self.assertRaisesRegex(ValueError, 'unknown original launcher cohort'):
                self.rows()

    def test_receipt_flag_cannot_override_raw_credit(self):
        row=copy.deepcopy(json.loads((SNAPSHOT/'stdlib/csv.jsonl').read_text().splitlines()[0]));row['paired']=False
        with self.assertRaisesRegex(ValueError,'paired does not match validated raw outcomes'):
            scorecard.validate_record(row,'stdlib',scorecard.families())


    def bind_temp(self, entry, binding, tmp):
        path = tmp/'binding.json'; path.write_text(json.dumps(binding)+'\n')
        entry['sources']['binding'] = locator(path); entry['files'][locator(path)] = digest(path)

    def artifact_temp(self, entry, cohort, role, path, data):
        path.write_bytes(data); cohort['artifacts'][role] = locator(path)
        entry['files'][locator(path)] = digest(path)

    def test_rehashed_paired_row_cannot_move_to_full_stock_cohort(self):
        import raw_control
        entry = self.entries['base64']; binding = json.loads((scorecard.ROOT/entry['sources']['binding']).read_text())
        rows = [json.loads(line) for line in (SNAPSHOT/'stdlib/base64.jsonl').read_text().splitlines()]
        row = next(r for r in rows if r['paired']); key = (row['id'],row['occurrence'])
        full = binding['cohorts']['stock-only']; run = json.loads((scorecard.ROOT/full['artifacts']['run']).read_text())
        event = raw_control.event_index(scorecard.ROOT/full['artifacts']['stock_stderr'], scorecard.require)[key]
        row['runs']['stock'].update(argv=run['runs']['stock']['argv'], evidence=full['artifacts']['stock_stderr']+'#L'+str(event['line']))
        next(r for r in binding['originals'] if (r['id'],r['occurrence'])==key)['launcher_cohort']='stock-only'
        with tempfile.TemporaryDirectory(dir=SNAPSHOT/'evidence/base64/raw-control') as folder:
            tmp=Path(folder); receipt=tmp/'base64.jsonl';receipt.write_text(''.join(json.dumps(r)+'\n' for r in rows))
            entry['receipt']=locator(receipt);entry['files'][locator(receipt)]=digest(receipt);self.bind_temp(entry,binding,tmp)
            with self.assertRaisesRegex(ValueError,'credited original cannot use a stock-only cohort'):
                scorecard.validate_record(row,'stdlib',scorecard.families())

    def test_rehashed_different_stock_launcher_archive_cannot_receive_credit(self):
        entry=self.entries['csv'];binding=json.loads((scorecard.ROOT/entry['sources']['binding']).read_text());cohort=binding['cohorts']['finite-native']
        with tempfile.TemporaryDirectory(dir=SNAPSHOT/'evidence/csv/raw-control') as folder:
            tmp=Path(folder); archive=tmp/'different.py'; original=(scorecard.ROOT/cohort['artifacts']['stock_launcher']).read_bytes()
            self.artifact_temp(entry,cohort,'stock_launcher',archive,original+b'\n# Different stock source\n')
            run=json.loads((scorecard.ROOT/cohort['artifacts']['run']).read_text());run['runs']['stock']['fresh_stock_launcher_sha256']=digest(archive)
            self.artifact_temp(entry,cohort,'run',tmp/'run.json',(json.dumps(run)+'\n').encode());self.bind_temp(entry,binding,tmp)
            with self.assertRaisesRegex(ValueError,'stock launcher bytes differ from native'):self.rows()

    def test_rehashed_stock_inventory_order_must_match_native(self):
        entry=self.entries['csv'];binding=json.loads((scorecard.ROOT/entry['sources']['binding']).read_text());cohort=binding['cohorts']['finite-native']
        with tempfile.TemporaryDirectory(dir=SNAPSHOT/'evidence/csv/raw-control') as folder:
            tmp=Path(folder);lines=(scorecard.ROOT/cohort['artifacts']['stock_stderr']).read_bytes().splitlines(keepends=True)
            lines[1],lines[2]=lines[2],lines[1];lines[129:133]=lines[131:133]+lines[129:131]
            raw=tmp/'stock.stderr';self.artifact_temp(entry,cohort,'stock_stderr',raw,b''.join(lines))
            run=json.loads((scorecard.ROOT/cohort['artifacts']['run']).read_text());run['runs']['stock']['stderr_sha256']=digest(raw)
            self.artifact_temp(entry,cohort,'run',tmp/'run.json',(json.dumps(run)+'\n').encode());self.bind_temp(entry,binding,tmp)
            with self.assertRaisesRegex(ValueError,'ordered stock/native inventories differ'):self.rows()

    def test_common_configuration_and_semantic_cwd(self):
        import raw_control
        entry=self.entries['csv'];binding=json.loads((scorecard.ROOT/entry['sources']['binding']).read_text());cohort=binding['cohorts']['finite-native']
        run=json.loads((scorecard.ROOT/cohort['artifacts']['run']).read_text());gate=json.loads((scorecard.ROOT/cohort['artifacts']['gate']).read_text());extension=scorecard.option(gate['command'],'--extension-path')
        run['runs']['stock']['cwd'] += '/.'
        raw_control.comparable_run(run,extension,gate['command'][0],scorecard.require)
        run['runs']['O0']['environment']['NO_COLOR']='0'
        with self.assertRaisesRegex(ValueError,'common configuration differs'):raw_control.comparable_run(run,extension,gate['command'][0],scorecard.require)

    def test_finite_and_stock_only_cohorts_preserve_base64_outcomes(self):
        entry=self.entries['base64'];binding=json.loads((scorecard.ROOT/entry['sources']['binding']).read_text());assign=Counter(r['launcher_cohort'] for r in binding['originals'])
        self.assertEqual(assign,{'finite-native':36,'stock-only':5})
        rows=[json.loads(line) for line in (SNAPSHOT/'stdlib/base64.jsonl').read_text().splitlines()]
        self.assertEqual(sum(r['paired'] for r in rows),35)
        lazy=next(r for r in rows if r['id'].endswith('LazyImportTest.test_lazy_import'))
        self.assertEqual([lazy['runs'][t]['outcome'] for t in scorecard.TIERS],['pass','skip','skip'])


if __name__ == '__main__': unittest.main()
