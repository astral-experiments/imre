"""Compare untouched StringIO consumers using the shared compiler-only startup probe."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

HERE = Path(__file__).resolve().parent
SUITES = ('test.test_csv', 'test.test_itertools', 'test.test_json')
PIN = '5607950ef232dad16d75c0cf53101d9649d89115'


def run(compiler, git):
    command = [sys.executable, '-B', HERE.parents[1] / 'check_suite_startup.py', '--compiler', compiler]
    if git is not None:
        command.extend(['--cpython-git', git])
    proc = subprocess.run([*command, *SUITES], stdout=subprocess.PIPE, check=False, timeout=120)
    report = json.loads(proc.stdout)
    expected = {(suite, entry, mode) for suite in SUITES for entry in ('import', 'runner') for mode in (False, True)}
    keys = [(row['suite'], row['entry'], row['reachable_only']) for row in report['checks']]
    if (len(keys) != len(expected) or set(keys) != expected or report['cpython_commit'] != PIN
            or report['operation'] != 'check' or report['tests_executed'] != 0
            or proc.returncode != int(any(row['exit'] != 0 for row in report['checks']))):
        raise ValueError(f'incomplete startup probe for {report.get("source")}: {report}')
    for row in report['checks']:
        row['diagnostic'] = re.sub(r'(?<= under )/[^;\n]*?/imre-cpython-suite-[^/\s]+/Lib(?=;)',
                                   '<staged>/Lib', row['diagnostic'])
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--imre', type=Path, required=True)
    parser.add_argument('--cpython-git', type=Path, required=True)
    parser.add_argument('--result', type=Path, required=True)
    args = parser.parse_args()
    offline, original = (run(args.imre, source) for source in (None, args.cpython_git))
    if offline['source'] != 'vendored' or original['source'] != 'git':
        raise ValueError('expected both vendored and full Git sources')
    if {k: v for k, v in offline.items() if k != 'source'} != {k: v for k, v in original.items() if k != 'source'}:
        raise ValueError(f'fixture sources differ: {offline!r} != {original!r}')
    offline['source'] = 'vendored and pinned Git Lib/test'
    offline['normalization'] = 'Only the randomly generated staging directory before /Lib is replaced by <staged>.'
    args.result.write_text(json.dumps(offline, indent=2) + '\n')
    for row in offline['checks']:
        status = 'compiled' if row['exit'] == 0 else 'blocked'
        print(f"{row['suite']} {row['entry']} reachable_only={row['reachable_only']}: {status}: {row['diagnostic']}")
    print('Compiler startup only; zero test bodies executed.')
    print('StringIO startup report:', json.dumps(offline, separators=(',', ':')))


if __name__ == '__main__':
    main()
