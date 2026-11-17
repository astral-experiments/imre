"""Exercise activated raw controls against the immutable merged snapshot."""
import copy
import hashlib
import json
import re
import tempfile
import unittest
from collections import Counter
from pathlib import Path
from unittest.mock import patch

import scorecard
import raw_control

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

    def test_gate_accepts_strace_pid_padding(self):
        entry = self.entries['csv']; binding = json.loads((scorecard.ROOT / entry['sources']['binding']).read_text())
        cohort = binding['cohorts']['finite-native']; art = cohort['artifacts']
        gate = json.loads((scorecard.ROOT / art['gate']).read_text())
        trace = (scorecard.ROOT / art['gate_trace']).read_text()
        padded = re.sub(r'^(\d+) ', r'\1  ', trace, flags=re.M)
        raw_control.trace_gate(padded, gate['command'], cohort['probe'], gate['first_party_module'],
                               (scorecard.ROOT / art['gate_stderr']).read_text(), scorecard.require)

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


if __name__ == '__main__': unittest.main()
