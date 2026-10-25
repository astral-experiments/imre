"""Compare the same supplemental source as a script and as an imported module."""

import argparse
import hashlib
import json
from pathlib import Path
import re
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tests'))
from paths import files, seed, snapshot

if not __debug__ or sys.platform != 'linux':
    raise SystemExit('the Linux entry report requires Python assertions on Linux')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('imre', 'runtime', 'clang', 'python', 'result'):
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--imre-commit', required=True)
    args = parser.parse_args()
    if not re.fullmatch('[0-9a-f]{40}', args.imre_commit):
        parser.error('supply the full measured Imre commit')
    oracle = str(args.python.resolve())
    version = files.run([oracle, '-I', '-c', 'import sys; print(sys.version_info[:3])'], ROOT)
    assert version.returncode == 0 and version.stdout.strip() == b'(3, 14, 5)'
    fixture = ROOT / 'tests/fixtures/paths/samefile/cases.py'
    with tempfile.TemporaryDirectory(prefix='pl-entry-') as tmp:
        work = Path(tmp)
        direct = work / 'original_samefile.py'
        direct.write_bytes(fixture.read_bytes())
        modes = {}
        for mode, source in [('direct', direct), ('import', fixture.with_name('main.py'))]:
            reference = work / (mode + '-cpython')
            seed(reference)
            expected = files.run([oracle, '-E', '-B', str(source)], reference)
            files.check_result(expected, None, 'CPython ' + mode)
            expected_files = snapshot(reference)
            levels = {}
            for level in ('0', '3'):
                binary = work / (mode + '-O' + level)
                build = files.run([str(args.imre.resolve()), 'build', str(source), '--module-root',
                                   str(source.parent), '-o', str(binary), '--opt-level', level, '--quiet',
                                   '--runtime', str(args.runtime.resolve()), '--clang', str(args.clang.resolve())], work)
                diagnostic = build.stderr.decode().replace(str(work), '<work>')
                row = levels[level] = {'compile_exit': build.returncode, 'diagnostic': diagnostic}
                if build.returncode:
                    assert mode == 'direct' and build.returncode == 1, (mode, level, diagnostic)
                    assert diagnostic == ('<work>/original_samefile.py:106:1: Imre: '
                                          'definitions must precede executable module statements\n'), diagnostic
                    continue
                assert not diagnostic, diagnostic
                native = work / (mode + '-run-' + level)
                seed(native)
                actual = files.run([str(binary)], native)
                row.update(exitcode=actual.returncode, stdout_matches=actual.stdout == expected.stdout,
                           stderr_matches=actual.stderr == expected.stderr,
                           filesystem_matches=snapshot(native) == expected_files)
                assert actual.returncode == expected.returncode and all(
                    row[key] for key in ('stdout_matches', 'stderr_matches', 'filesystem_matches')), row
            modes[mode] = {'reference_lines': len(expected.stdout.splitlines()), 'levels': levels}
    reference = json.loads(Path(__file__).with_name('reference-linux.json').read_bytes())
    report = {'schema': 'imre.cpython-stdlib.pathlib.supplemental-entry/v1',
              'cpython_commit': reference['cpython_commit'], 'platform': sys.platform,
              'imre_commit': args.imre_commit, 'source': str(fixture.relative_to(ROOT)),
              'contributes_upstream_occurrences': 0,
              'source_sha256': hashlib.sha256(fixture.read_bytes()).hexdigest(), 'modes': modes}
    args.result.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    for mode, data in modes.items():
        print(mode, data['reference_lines'], {level: row['compile_exit'] for level, row in data['levels'].items()})


if __name__ == '__main__':
    main()
