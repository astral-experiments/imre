"""Reject stale source information in recorded patch.dict blockers."""

from copy import deepcopy
from pathlib import Path
import sys
import unittest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[1]))
from patch_cases import known_blockers, recorded_blocker


class BlockerIdentityTests(unittest.TestCase):
    def test_compile_observation_keeps_its_own_identity(self):
        name = 'test.test_unittest.testmock.testpatch.PatchTest.test_patch_dict'
        for reachable in (False, True):
            for level in (0, 3):
                prior = known_blockers(reachable, level)[name, 1]
                method = prior['evidence']['definition_sha256']
                source = dict(stage='compile', source_blob=prior['evidence']['source_blob'])
                row = dict(id=name, occurrence=1, outcome='blocked', reason=prior['reason'],
                           evidence=dict(source, method_sha256=method))
                self.assertTrue(recorded_blocker(deepcopy(row), prior, method, HERE))
                for change in (dict(evidence=source), dict(evidence=dict(source, method_sha256='0' * 64)),
                               dict(id=name + '_other'), dict(occurrence=2)):
                    with self.subTest(reachable=reachable, level=level, change=change):
                        self.assertFalse(recorded_blocker(dict(deepcopy(row), **change), prior, method, HERE))

    def test_unstaged_fixture_records_the_pinned_definition(self):
        prior = next(r for r in known_blockers(False, 0).values() if r['evidence']['stage'] == 'original fixture')
        method = prior['evidence']['definition_sha256']
        row = dict(id=prior['id'], occurrence=prior['occurrence'], outcome='blocked', reason=prior['reason'],
                   evidence=dict(stage='original fixture', source_blob=prior['evidence']['source_blob']))
        self.assertTrue(recorded_blocker(row, prior, method, HERE))
        self.assertEqual(row['evidence']['method_sha256'], method)
        row['evidence']['method_sha256'] = '0' * 64
        self.assertFalse(recorded_blocker(row, prior, method, HERE))


if __name__ == '__main__':
    unittest.main()
