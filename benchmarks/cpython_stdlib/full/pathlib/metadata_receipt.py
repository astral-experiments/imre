"""Check or update measured filesystem metadata methods in the full report."""

import argparse
from collections import Counter
import json
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
sys.path[:0] = [str(HERE), str(HERE.parents[1])]
from native_metadata import METHODS, SYMLINK_METHODS
from native_stat import FIELD_ASSERTIONS, FIELD_CALLS, METHODS as STAT_METHODS
from suite_evidence import validate_pair, write_reports


def without_revision(row):
    return {**row, 'evidence': {k: v for k, v in row['evidence'].items() if k != 'imre_commit'}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('measured', type=Path)
    parser.add_argument('--baseline', type=Path, default=HERE / 'evidence-linux.json')
    parser.add_argument('--result', type=Path)
    parser.add_argument('--imre-commit')
    group = parser.add_mutually_exclusive_group()
    group.add_argument('--links', action='store_true', help='select the four upstream symlink and readlink occurrences')
    group.add_argument('--stat', action='store_true', help='select the ten upstream stat and lstat occurrences')
    args = parser.parse_args()
    if (bool(args.result) != bool(args.imre_commit)
            or args.imre_commit and not re.fullmatch(r'[0-9a-f]{40}', args.imre_commit)):
        parser.error('an update requires both an output path and the full measured compiler commit')
    reference = json.loads((HERE / 'reference-linux.json').read_bytes())
    baseline = json.loads(args.baseline.read_bytes())
    measurements = json.loads(args.measured.read_bytes())
    for pair in (baseline, measurements):
        validate_pair(pair, reference)
    methods = STAT_METHODS if args.stat else SYMLINK_METHODS if args.links else METHODS
    ids = {f'test.test_pathlib.test_pathlib.{cls}.{name}'
           for cls in ('PathTest', 'PosixPathTest') for name in methods}
    for report in baseline:
        level = report['opt_level']
        measured = next(r for r in measurements if r['opt_level'] == level)
        rows = {r['id']: r for r in measured['records'] if r['id'] in ids}
        if rows.keys() != ids:
            raise ValueError(f'missing metadata methods at O{level}')
        for i, old in enumerate(report['records']):
            if old['id'] not in ids:
                continue
            new = rows[old['id']]
            driven = args.stat and new['id'].endswith('.test_stat')
            body = 'cpython' if driven else 'imre'
            if (new['classification'] != ('host-driven-native-operation' if driven else 'native-body-host-fixture')
                    or new['roles'] != {'body': body, 'assertions': body, 'operation': 'imre',
                                        'fixture': 'cpython' if driven else 'mixed'}
                    or driven and (new['evidence'].get('calls') != FIELD_CALLS
                                   or new['evidence'].get('host_assertions') != FIELD_ASSERTIONS)
                    or new['evidence'].get('host_fixture_hooks') != ['setUp', 'tearDown']
                    or new['evidence']['method_sha256'] != old['evidence']['method_sha256']):
                raise ValueError(f'unverified method, operation or fixture: {new["id"]} at O{level}')
            if args.result:
                measured_commit = new['evidence'].get('imre_commit')
                if measured_commit and measured_commit != args.imre_commit:
                    raise ValueError(f'measured compiler mismatch for {new["id"]}')
                new['evidence']['imre_commit'] = args.imre_commit
                report['records'][i] = new
            else:
                # A new compiler revision should produce the same semantic receipt.
                if without_revision(old) != without_revision(new):
                    raise ValueError(f'different observation for {new["id"]} at O{level}')
        report['counts'] = dict(sorted(Counter(r['classification'] for r in report['records']).items()))
        label = 'stat/lstat' if args.stat else 'link' if args.links else 'Path.info'
        print(f'O{level}: {len(rows)} unchanged {label} methods; CPython setup/teardown; '
              f'{Counter(r["classification"] for r in rows.values())}')
    if args.result:
        write_reports(args.result, baseline, reference)


if __name__ == '__main__':
    main()
