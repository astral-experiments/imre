"""Stock source archives must bind to the immutable CPython original."""
import copy
import hashlib
import io
import json
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

import scorecard

PIN = '8571ca2dbecabf8d7863c369dc776611fd57d8ad'
SNAPSHOT = scorecard.HERE / 'revisions' / PIN


def sha(data): return hashlib.sha256(data).hexdigest()
def locator(path): return path.relative_to(scorecard.ROOT).as_posix()


class StockSourceTests(unittest.TestCase):
    def setUp(self):
        activation = json.loads((SNAPSHOT / 'activation.json').read_text())
        self.entry = copy.deepcopy(activation['families']['dataclasses'])
        self.binding = json.loads((scorecard.ROOT / self.entry['sources']['binding']).read_text())
        self.cohort = self.binding['cohorts']['finite-native']
        self.row = json.loads((scorecard.ROOT / self.entry['receipt']).read_text().splitlines()[0])
        self.source = self.row['source']
        self.capture = json.loads((scorecard.ROOT / self.cohort['artifacts']['stock_capture']).read_text())
        self.original = (scorecard.ROOT / self.cohort['artifacts']['stock_source']).read_bytes()
        self.addCleanup(patch.stopall)
        patch.object(scorecard, 'DATA_ROOT', SNAPSHOT).start()
        patch.object(scorecard, 'COHORT', {**scorecard.CANONICAL_COHORT, 'imre_revision': PIN}).start()
        patch.object(scorecard, 'snapshot_activation', return_value={'families': {'dataclasses': self.entry}}).start()
        self.temp = tempfile.TemporaryDirectory(dir=SNAPSHOT / 'evidence/dataclasses/raw-control')
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)

    def artifact(self, role, data):
        path = self.work / role
        path.write_bytes(data)
        self.cohort['artifacts'][role] = locator(path)
        self.entry['files'][locator(path)] = sha(data)

    def reseal(self):
        self.artifact('stock_capture', (json.dumps(self.capture) + '\n').encode())
        path = self.work / 'binding.json'
        path.write_text(json.dumps(self.binding) + '\n')
        self.entry['sources']['binding'] = locator(path)
        self.entry['files'][locator(path)] = sha(path.read_bytes())

    def validate(self):
        scorecard.validate_record(self.row, 'stdlib', scorecard.families())

    def archive(self, kind, member, data):
        buf = io.BytesIO()
        if kind == 'tar':
            with tarfile.open(fileobj=buf, mode='w:gz') as archive:
                info = tarfile.TarInfo(member); info.size = len(data)
                archive.addfile(info, io.BytesIO(data))
        else:
            with zipfile.ZipFile(buf, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
                archive.writestr(member, data)
        return buf.getvalue()

    def test_rehashed_wrong_file_cannot_replace_immutable_source(self):
        wrong = b'# attacker-controlled source\n'
        self.artifact('stock_source', wrong)
        self.capture['source_archive_sha256'] = sha(wrong)  # Reseal every mutable hash.
        self.reseal()
        with self.assertRaisesRegex(ValueError, 'stock source archive member differs from the pinned source'):
            self.validate()

    def test_nested_tar_and_zip_member_bytes_are_anchored(self):
        member = f"cpython-{self.source['commit']}/{self.source['path']}"
        for kind in ('tar', 'zip'):
            with self.subTest(kind=kind, content='original'):
                data = self.archive(kind, member, self.original)
                self.artifact('stock_source', data); self.cohort['stock_source_member'] = member
                self.capture.update(source_archive_sha256=sha(data), source_member=member)
                self.reseal(); self.validate()
            with self.subTest(kind=kind, content='tampered'):
                data = self.archive(kind, member, self.original + b'\n# tamper\n')
                self.artifact('stock_source', data); self.capture['source_archive_sha256'] = sha(data)
                self.reseal()
                with self.assertRaisesRegex(ValueError, 'stock source archive member differs from the pinned source'):
                    self.validate()

    def test_member_path_cannot_remap_to_another_commit(self):
        member = f"cpython-{'0'*40}/{self.source['path']}"
        data = self.archive('tar', member, self.original)
        self.artifact('stock_source', data); self.cohort['stock_source_member'] = member
        self.capture.update(source_archive_sha256=sha(data), source_member=member)
        self.reseal()
        with self.assertRaisesRegex(ValueError, 'member path differs from the pinned source'):
            self.validate()

    def test_capture_commit_hash_and_nested_identity_cannot_be_rehashed(self):
        mutations = (
            {'source_commit': '0'*40},
            {'source_archive_sha256': '0'*64},
            {'source': {**self.source, 'sha256': '0'*64}},
            {'source_sha256': '0'*64},
        )
        baseline = copy.deepcopy(self.capture)
        for mutation in mutations:
            with self.subTest(mutation=mutation):
                self.capture = {**baseline, **mutation}; self.reseal()
                with self.assertRaisesRegex(ValueError, 'stock source identity or archive digest differs'):
                    self.validate()

    def test_row_commit_and_hash_remain_independently_frozen(self):
        for field, value in (('commit', '0'*40), ('sha256', '0'*64)):
            with self.subTest(field=field):
                row = copy.deepcopy(self.row); row['source'][field] = value
                with self.assertRaisesRegex(ValueError, 'wrong CPython source|source differs from canonical inventory'):
                    scorecard.validate_record(row, 'stdlib', scorecard.families())

    def test_source_role_cannot_be_removed(self):
        del self.cohort['artifacts']['stock_source']; self.reseal()
        with self.assertRaisesRegex(ValueError, 'missing raw-control role: stock_source'):
            self.validate()


if __name__ == '__main__': unittest.main()
