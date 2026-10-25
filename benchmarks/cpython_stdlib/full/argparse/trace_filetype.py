"""Measure the real files and stream identities used by the original FileType tests."""

import argparse
from collections import Counter, defaultdict
import json
from pathlib import Path
import sys
import unittest


def measure(suite_dir):
    if sys.version_info[:3] != (3, 14, 5) or 'test.test_argparse' in sys.modules:
        raise RuntimeError('start a fresh CPython 3.14.5 process')
    sys.path.insert(0, str(suite_dir.parents[1]))
    from suite_inventory import cases, load_suite, verify_files

    manifest = json.loads((suite_dir / 'UPSTREAM.json').read_bytes())
    verify_files(suite_dir / 'upstream', manifest['files'])
    origin = Path(argparse.__file__).resolve().parents[1]
    verify_files(origin, manifest['files'] | manifest['reference_files'])

    active = 'module import'
    calls = {getattr(argparse.FileType, name).__code__: name
             for name in ('__init__', '__call__', '__repr__')}
    comparers = {}
    counts = defaultdict(Counter)

    def trace(frame, event, arg):
        if event != 'call' or (method := calls.get(frame.f_code)) is None:
            return None
        row = counts[active]
        row[method] += 1
        if method != '__call__':
            return None
        row['mode:' + frame.f_locals['self']._mode] += 1
        last = None

        def conversion(frame, event, result):
            nonlocal last
            if event == 'exception':
                last = result[0].__name__
                row['raised:' + last] += 1
            elif event == 'return':
                if result is None:
                    if last is None:
                        raise AssertionError('unexpected normal return of None')
                    row['outcome:raise ' + last] += 1
                else:
                    for stream in ('stdin', 'stdout', 'stderr'):
                        value = getattr(sys, stream)
                        if result is value or result is getattr(value, 'buffer', None):
                            row['outcome:' + stream + ('' if result is value else '.buffer')] += 1
                            break
                    else:
                        row['outcome:' + type(result).__module__ + '.' + type(result).__qualname__] += 1
            return conversion
        return conversion

    def profile(frame, event, method):
        owner = comparers.get(frame.f_code)
        if owner is None:
            return
        row = counts[active]
        if event == 'call':
            row[owner + '.__eq__'] += 1
        if (event in ('c_return', 'c_exception') and
                getattr(method, '__self__', None) is frame.f_locals['other'] and
                method.__name__ in ('read', 'write', 'close')):
            row[owner + '.' + method.__name__ + (':raised' if event == 'c_exception' else '')] += 1

    class Result(unittest.TestResult):
        def startTest(self, case):
            nonlocal active
            super().startTest(case)
            active = case.id()

        def stopTest(self, case):
            nonlocal active
            super().stopTest(case)
            active = 'class or module fixture'

    old_trace, old_profile = sys.gettrace(), sys.getprofile()
    try:
        sys.settrace(trace)
        from test import test_argparse as original
        if Path(original.__file__).resolve() != origin / 'Lib/test/test_argparse.py':
            raise ValueError('the imported test module does not match the verified source')
        comparers.update((cls.__eq__.__code__, cls.__name__)
                         for cls in (original.RFile, original.WFile, original.StdStreamComparer))
        all_cases = list(cases(load_suite(manifest['module'])))
        all_ids = {case.id() for case in all_cases}
        reference = json.loads((suite_dir / f'reference-{sys.platform}.json').read_bytes())
        if (len(all_cases) != reference['denominator'] or all_ids != set(reference['reference'])
                or set(reference['reference'].values()) != {'pass'}):
            raise ValueError('discovery differs from the complete reference inventory')
        selected = [c for c in all_cases if type(c).__name__.startswith('TestFileType')]
        active = 'class or module fixture'
        sys.setprofile(profile)
        result = Result()
        unittest.TestSuite(all_cases).run(result)
    finally:
        sys.setprofile(old_profile)
        sys.settrace(old_trace)
    if (result.testsRun != len(all_cases) or result.failures or result.errors or result.skipped
            or result.expectedFailures or result.unexpectedSuccesses):
        raise RuntimeError((result.testsRun, result.failures, result.errors, result.skipped))
    rows = {c.id(): dict(sorted(counts[c.id()].items())) for c in selected}
    total = Counter()
    for row in rows.values():
        total.update(row)
    outside = {case.id(): dict(sorted(counts[case.id()].items())) for case in all_cases
               if case.id() not in rows and counts.get(case.id())}
    return {'execution': 'CPython reference only', 'native_test_occurrences': 0,
            'cpython_commit': manifest['commit'], 'source_blobs': manifest['files'] | manifest['reference_files'],
            'denominator': len(all_cases), 'suite_reference_passes': result.testsRun,
            'selected': len(rows), 'reference_passes': len(rows),
            'counts_during_test_occurrences': dict(sorted(total.items())),
            'observations_in_other_occurrences': outside,
            'counts_outside_test_occurrences': {k: dict(sorted(v.items())) for k, v in counts.items()
                                               if k not in all_ids},
            'occurrences': rows}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--suite-dir', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--result', type=Path, required=True)
    args = parser.parse_args()
    report = measure(args.suite_dir.resolve())
    args.result.write_text(json.dumps(report, indent=2) + '\n')
    print(f"{report['reference_passes']}/{report['selected']} FileType reference cases; "
          f"{report['denominator']} total; zero native occurrences")
