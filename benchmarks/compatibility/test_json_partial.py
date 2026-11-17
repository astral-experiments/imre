"""Partial JSON activation keeps staged observations outside strict totals."""
import copy, hashlib, json, tempfile, unittest
from pathlib import Path
from unittest.mock import patch
import scorecard
PIN = '8571ca2dbecabf8d7863c369dc776611fd57d8ad'
SNAP = scorecard.HERE / 'revisions' / PIN
L = lambda p: p.relative_to(scorecard.ROOT).as_posix()
H = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()

class PartialJsonTests(unittest.TestCase):

    def setUp(self):
        self.addCleanup(patch.stopall)
        patch.object(scorecard, 'DATA_ROOT', SNAP).start()
        patch.object(scorecard, 'COHORT', {**scorecard.CANONICAL_COHORT, 'imre_revision': PIN}).start()
        self.act = copy.deepcopy(scorecard.snapshot_activation())
        self.entry = self.act['families']['json']
        patch.object(scorecard, 'snapshot_activation', return_value=self.act).start()
        self.binding = json.loads((scorecard.ROOT / self.entry['sources']['binding']).read_text())
        self.staged = [json.loads(l) for l in (SNAP / 'stdlib/json.jsonl').read_text().splitlines()]
        self.verified = [json.loads(l) for l in (SNAP / 'verified/stdlib/json.jsonl').read_text().splitlines()]
        self.tmp = tempfile.TemporaryDirectory(dir=SNAP / 'evidence/json/strict')
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)

    def put(self, name, data):
        p = self.dir / name
        p.write_bytes(data if isinstance(data, bytes) else data.encode())
        self.entry['files'][L(p)] = H(p)
        return L(p)

    def bind(self):
        self.entry['sources']['binding'] = self.put('binding.json', json.dumps(self.binding))

    def validate(self, r):
        return scorecard.validate_record(r, 'stdlib', scorecard.families())

    def test_real_partial_activation_counts_only_verified_subset(self):
        fs, _ = scorecard.summary()
        j = fs['json']
        self.assertEqual(j['receipts'], len(self.verified))
        self.assertEqual(j['paired'], len(self.verified))
        self.assertEqual(j['pending'], 216 - len(self.verified))
        self.assertEqual(len(self.staged), 216)
        self.assertEqual(sum(('.test_tool.' in r['id'] and r['runs']['O0']['outcome'] == 'timeout' for r in self.staged)), 34)
        self.assertEqual(sum((r['runs']['O0']['outcome'] == 'timeout' for r in self.staged)), 36)

    def test_rehashed_staged_to_finite_promotion_fails(self):
        active = {(r['id'], r['occurrence']) for r in self.verified}
        row = copy.deepcopy(next((r for r in self.staged if 'test_unicode' in r['id'] and (r['id'], r['occurrence']) not in active)))
        row['paired'] = True
        for t in scorecard.TIERS:
            row['runs'][t]['outcome'] = 'pass'
        receipt = self.put('fake-receipt.jsonl', ''.join((json.dumps(r) + '\n' for r in [*self.verified, row])))
        self.entry['receipt'] = receipt
        self.binding['originals'].append({'id': row['id'], 'occurrence': row['occurrence'], 'launcher_cohort': 'finite-unicode'})
        self.bind()
        with self.assertRaisesRegex(ValueError, 'launcher cohort stock inventory does not cover its assigned originals'):
            self.validate(row)

    def test_rehashed_native_revision_fails(self):
        row = next((r for r in self.verified if 'test_unicode' in r['id']))
        a = self.binding['cohorts']['finite-unicode']['artifacts']
        run = json.loads((scorecard.ROOT / a['run']).read_text())
        run['revision'] = '0' * 40
        a['run'] = self.put('wrong-revision.json', json.dumps(run))
        self.bind()
        with self.assertRaisesRegex(ValueError, 'source, platform or revision differs'):
            self.validate(row)

    def make_partial_fixture(self, root, staged_rows, verified_rows):
        staged = root / 'stdlib/json.jsonl'
        verified = root / 'verified/stdlib/json.jsonl'
        staged.parent.mkdir(parents=True, exist_ok=True)
        verified.parent.mkdir(parents=True, exist_ok=True)
        staged.write_text(''.join((json.dumps(r) + '\n' for r in staged_rows)))
        verified.write_text(''.join((json.dumps(r) + '\n' for r in verified_rows)))
        entry = {'receipt': L(verified), 'staged_receipt': L(staged), 'files': {L(staged): H(staged), L(verified): H(verified)}}
        return (staged, verified, entry)

    def test_rehashed_invented_staged_id_fails(self):
        with tempfile.TemporaryDirectory(dir=scorecard.ROOT) as tmp:
            root = Path(tmp)
            invented = copy.deepcopy(self.staged[10])
            invented['id'] = 'test.test_json.invented.original'
            staged, _, entry = self.make_partial_fixture(root, [invented], [])
            with patch.object(scorecard, 'DATA_ROOT', root):
                with self.assertRaisesRegex(ValueError, 'original ID, occurrence or source differs'):
                    scorecard.activated_stdlib_path(staged, entry)

    def test_rehashed_known_staged_id_with_wrong_source_fails(self):
        with tempfile.TemporaryDirectory(dir=scorecard.ROOT) as tmp:
            root = Path(tmp)
            wrong = copy.deepcopy(next((r for r in self.staged if 'test_unicode' in r['id'])))
            wrong['source']['sha256'] = '0' * 64
            staged, _, entry = self.make_partial_fixture(root, [wrong], [])
            with patch.object(scorecard, 'DATA_ROOT', root):
                with self.assertRaisesRegex(ValueError, 'source differs from canonical inventory'):
                    scorecard.activated_stdlib_path(staged, entry)

    def test_rehashed_forged_staged_claim_cannot_change_validated_totals(self):
        verified = self.verified[0]
        staged_rows = copy.deepcopy(self.staged)
        pending = next((r for r in staged_rows if '.test_tool.' in r['id']))
        with tempfile.TemporaryDirectory(dir=scorecard.ROOT) as tmp:
            root = Path(tmp)
            staged, _, entry = self.make_partial_fixture(root, staged_rows, [verified])
            choose = lambda name, kind='stdlib': entry if name == 'json' and kind == 'stdlib' else None
            with patch.object(scorecard, 'DATA_ROOT', root), patch.object(scorecard, 'activation_entry', side_effect=choose), patch.object(scorecard, 'load_records', side_effect=lambda kind, *args: [verified] if kind == 'stdlib' else []):
                before = scorecard.summary()[0]['json']
                pending['paired'] = True
                for t in scorecard.TIERS:
                    pending['runs'][t]['outcome'] = 'pass'
                staged.write_text(''.join((json.dumps(r) + '\n' for r in staged_rows)))
                entry['files'][L(staged)] = H(staged)
                after = scorecard.summary()[0]['json']
                self.assertEqual((before['receipts'], before['paired'], before['pending']), (after['receipts'], after['paired'], after['pending']))
                self.assertEqual((after['receipts'], after['paired'], after['pending']), (1, 1, 215))
                html = scorecard.render('readme')
                self.assertNotIn('raw:', html)
                self.assertIn('staged (unverified; outcomes not checked)', html)

    def test_rehashed_truncated_staged_shard_fails(self):
        verified = {(r['id'], r['occurrence']) for r in self.verified}
        pending = next(r for r in self.staged if (r['id'], r['occurrence']) not in verified)
        rows = [r for r in self.staged if (r['id'], r['occurrence']) != (pending['id'], pending['occurrence'])]
        with tempfile.TemporaryDirectory(dir=scorecard.ROOT) as tmp:
            root = Path(tmp)
            _, _, entry = self.make_partial_fixture(root, rows, self.verified)
            choose = lambda name, kind='stdlib': entry if name == 'json' and kind == 'stdlib' else None
            with patch.object(scorecard, 'DATA_ROOT', root), patch.object(scorecard, 'activation_entry', side_effect=choose), patch.object(scorecard, 'load_records', side_effect=lambda kind, *args: self.verified if kind == 'stdlib' else []):
                with self.assertRaisesRegex(ValueError, 'partial staged shard does not cover frozen denominator'):
                    scorecard.summary()

    def test_rehashed_malformed_stock_capture_fails(self):
        row = next((r for r in self.verified if 'test_unicode' in r['id']))
        a = self.binding['cohorts']['finite-unicode']['artifacts']
        raw = (scorecard.ROOT / a['stock_stderr']).read_text().splitlines()
        line = int(row['runs']['stock']['evidence'].rsplit('#L', 1)[1])
        raw[line - 1] = 'IMRE_UNITTEST {malformed'
        data = ('\n'.join(raw) + '\n').encode()
        a['stock_stderr'] = self.put('malformed.stderr', data)
        digest = hashlib.sha256(data).hexdigest()
        run = json.loads((scorecard.ROOT / a['run']).read_text())
        run['runs']['stock']['stderr_sha256'] = digest
        a['run'] = self.put('malformed-run.json', json.dumps(run))
        capture = json.loads((scorecard.ROOT / a['stock_capture']).read_text())
        capture['stderr_sha256'] = digest
        a['stock_capture'] = self.put('malformed-capture.json', json.dumps(capture))
        self.bind()
        with self.assertRaises(ValueError):
            self.validate(row)

    def test_rehashed_launcher_change_cannot_replace_gate(self):
        row = next((r for r in self.verified if 'test_unicode' in r['id']))
        a = self.binding['cohorts']['finite-unicode']['artifacts']
        data = (scorecard.ROOT / a['positive_launcher']).read_bytes() + b'print("tampered")\n'
        a['positive_launcher'] = a['stock_launcher'] = self.put('launcher.py', data)
        run = json.loads((scorecard.ROOT / a['run']).read_text())
        run['launcher_sha256'] = hashlib.sha256(data).hexdigest()
        a['run'] = self.put('launcher-run.json', json.dumps(run))
        capture = json.loads((scorecard.ROOT / a['stock_capture']).read_text())
        capture['launcher_sha256'] = run['launcher_sha256']
        a['stock_capture'] = self.put('launcher-capture.json', json.dumps(capture))
        self.bind()
        with self.assertRaisesRegex(ValueError, 'launcher code delta differs'):
            self.validate(row)

    def test_rehashed_cohort_swap_cannot_inherit_controls(self):
        row = next((r for r in self.verified if 'test_unicode' in r['id']))
        m = next((x for x in self.binding['originals'] if (x['id'], x['occurrence']) == (row['id'], row['occurrence'])))
        m['launcher_cohort'] = 'finite-decode'
        self.bind()
        with self.assertRaisesRegex(ValueError, 'source, platform or revision differs'):
            self.validate(row)

    def test_rehashed_wrong_source_archive_fails(self):
        row = next((r for r in self.verified if 'test_unicode' in r['id']))
        a = self.binding['cohorts']['finite-unicode']['artifacts']
        a['stock_source'] = self.put('wrong-source.py', b'not the original source\n')
        capture = json.loads((scorecard.ROOT / a['stock_capture']).read_text())
        capture['source_archive_sha256'] = self.entry['files'][a['stock_source']]
        a['stock_capture'] = self.put('source-capture.json', json.dumps(capture))
        self.bind()
        with self.assertRaises(ValueError):
            self.validate(row)
if __name__ == '__main__':
    unittest.main()
