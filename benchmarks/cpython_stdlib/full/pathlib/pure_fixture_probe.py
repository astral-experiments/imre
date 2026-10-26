"""Exercise the registered CPython pure-path fixtures, including their flavour skips."""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
from pathlib import Path
import posixpath
import sys
import unittest

HERE = Path(__file__).resolve().parent
BENCH = HERE.parents[1]
sys.path.insert(0, str(BENCH))
from suite_evidence import occurrence_keys, reference_cases, validate_pair
from suite_inventory import cases, load_suite, verify_files
from suite_native import NativeFixtureError, _default_source, method_bytes
from upstream import verify_upstream

FAMILIES = ('PurePathTest', 'PurePosixPathTest', 'PureWindowsPathTest')
SOURCE = 'Lib/test/test_pathlib/test_pathlib.py'


def check_fixture(case, upstream):
    """Execute the original hook and compare it with the live decorator registrations."""
    name, parser = case._testMethodName, case.cls.parser
    expected = None
    if name in upstream._tests_needing_posix and parser is not posixpath:
        expected = 'requires POSIX-flavoured path class'
    elif name in upstream._tests_needing_windows and parser is posixpath:
        expected = 'requires Windows-flavoured path class'
    if type(case).setUp is not upstream.PurePathTest.setUp or type(case).tearDown is not unittest.TestCase.tearDown:
        raise ValueError(f'different fixture for {case.id()}')
    try:
        case.setUp()
    except unittest.SkipTest as exc:
        if expected != str(exc):
            raise ValueError(f'unexpected skip for {case.id()}: {exc}')
        return {'skip': str(exc)}
    try:
        if expected or case.parser is not parser or (case.sep, case.altsep) != (parser.sep, parser.altsep):
            raise ValueError(f'different fixture state for {case.id()}')
        return {'parser': parser.__name__, 'sep': case.sep, 'altsep': case.altsep}
    finally:
        case.tearDown()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--result', type=Path, required=True)
    parser.add_argument('--candidate-manifest', type=Path, help='cross-check the coordinator selection')
    args = parser.parse_args()
    if sys.implementation.name != 'cpython' or sys.version_info[:3] != (3, 14, 5):
        parser.error('use the test-enabled pinned CPython 3.14.5')
    from test import test_pathlib
    from test.test_pathlib import test_pathlib as upstream
    commit = verify_upstream()
    files = json.loads((BENCH / 'UPSTREAM.json').read_bytes())['files']
    package = Path(test_pathlib.__file__).resolve().parent
    original = {p.removeprefix('Lib/test/test_pathlib/'): blob for p, blob in files.items()
                if p.startswith('Lib/test/test_pathlib/')}
    verify_files(package, original)
    if {p.relative_to(package).as_posix() for p in package.rglob('*.py')} != set(original):
        raise ValueError('the interpreter test package has different modules')
    reference = json.loads((HERE / 'reference-linux.json').read_bytes())
    reports = json.loads((HERE / 'evidence-linux.json').read_bytes())
    validate_pair(reports, reference)
    baseline = reference_cases(reference)
    registered = list(cases(load_suite(reference['module'])))
    all_keys = list(occurrence_keys(case.id() for case in registered))
    if set(all_keys) != set(baseline) or reference['cpython_commit'] != commit or sys.platform != reference['platform']:
        raise ValueError('full reference discovery or platform differs')
    by_level = [{(row['id'], row['occurrence']) for row in report['records']
                 if row['classification'] == 'native-body-host-fixture'} for report in reports]
    if len(by_level) != 2 or by_level[0] != by_level[1]:
        raise ValueError('candidate sets differ by optimization level')
    eligible = by_level[0]
    if args.candidate_manifest:
        index = json.loads(args.candidate_manifest.read_bytes())
        keys = [(r['id'], r['occurrence']) for r in index['cases']['pathlib']]
        if len(keys) != len(eligible) or set(keys) != eligible or index['upstream'] != commit:
            raise ValueError('coordinator and local candidate sets differ')
    families = {getattr(upstream, name) for name in FAMILIES}
    selected = [(key, case) for key, case in zip(all_keys, registered, strict=True) if type(case) in families]
    result = unittest.TestResult()
    unittest.TestSuite(case for _, case in selected).run(result)
    if (result.testsRun != len(selected) or not result.wasSuccessful() or result.expectedFailures
            or result.unexpectedSuccesses):
        raise ValueError(f'original suite unsuccessful: {result.failures}, {result.errors}')
    skipped = {id(case): reason for case, reason in result.skipped}
    fresh = list(cases(load_suite(reference['module'])))
    if [c.id() for c in registered] != [c.id() for c in fresh]:
        raise ValueError('fresh discovery differs')
    fresh = dict(zip(all_keys, fresh, strict=True))
    path = BENCH / 'upstream' / SOURCE
    rows, names = [], defaultdict(set)
    for key, case in selected:
        name, occurrence = key
        if name != unittest.TestCase.id(case) or name != f'{upstream.__name__}.{type(case).__name__}.{case._testMethodName}':
            raise ValueError(f'unexpected registered identity: {key}')
        actual = {'skipped': skipped[id(case)]} if id(case) in skipped else 'pass'
        if actual != baseline[key]:
            raise ValueError(f'{key}: {actual} differs from reference {baseline[key]}')
        clean_case = fresh[key]
        rejection = None
        if key in eligible:
            names[type(case).__name__].add(case._testMethodName)
            try:
                _default_source(clean_case, path, files[SOURCE])
            except NativeFixtureError as exc:
                rejection = str(exc).removeprefix(name + ': ')
            else:
                raise ValueError(f'{key}: native staging now accepts this fixture; run it before promotion')
        hook = check_fixture(clean_case, upstream)
        if ('skip' in hook) != (actual != 'pass') or 'skip' in hook and hook['skip'] != actual['skipped']:
            raise ValueError(f'{key}: original fixture and whole case disagree')
        row = {'id': name, 'occurrence': occurrence, 'cpython_case': actual, 'cpython_fixture': hook,
               'native_body_candidate': key in eligible}
        if rejection:
            row['shared_runner'] = rejection
        rows.append(row)
    totals = {f: {'registered': sum(type(c).__name__ == f for _, c in selected),
                  'candidates': len(names[f]),
                  'fixture_skips': dict(Counter(r['cpython_fixture']['skip'] for r in rows
                                     if r['id'].rsplit('.', 2)[1] == f and 'skip' in r['cpython_fixture']))}
              for f in FAMILIES}
    record = {'schema': 'imre.pathlib.registered-pure-fixtures/v1', 'cpython_commit': commit,
              'platform': sys.platform, 'source': SOURCE, 'source_blob': files[SOURCE],
              'setup_sha256': hashlib.sha256(method_bytes(path, 'PurePathTest', 'setUp')).hexdigest(),
              'package_denominator': len(all_keys), 'package_native_body_host_fixture': len(eligible),
              'registered_cases': len(rows), 'reference_passes': len(rows)-len(skipped), 'reference_skips': len(skipped),
              'candidate_count': sum(r['native_body_candidate'] for r in rows),
              'common_candidate_methods': sorted(set.intersection(*(names[f] for f in FAMILIES))),
              'families': totals, 'fully_native_original_cases_observed': 0, 'records': rows}
    args.result.write_text(json.dumps(record, indent=2) + '\n', encoding='utf-8')
    print(f'{len(rows)} registered originals; {len(rows)-len(skipped)} passed, {len(skipped)} skipped; '
          f'{record["candidate_count"]} candidates, {len(record["common_candidate_methods"])} shared names')


if __name__ == '__main__':
    main()
