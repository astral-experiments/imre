"""Run complete native pathlib methods under the pinned suite and write v1 evidence."""

import argparse
from contextlib import chdir
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
BENCHMARK = HERE.parents[1]
sys.path[:0] = [str(HERE), str(BENCHMARK)]
from suite_evidence import reference_cases, roles, write_reports
from suite_inventory import load_suite, verify_interpreter_tests
from suite_native import Toolchain, run_evidence
from upstream import verify_upstream
from body_report import report_blockers
import native_lexical
from native_basics import basic_adapters
from native_traversal import traversal_adapters
from native_components import component_adapters
from native_walk import walk_adapters
from native_metadata import metadata_adapters
from native_stat import stat_adapters
from native_segments import segment_adapters
from native_samefile import samefile_adapters


def skip_source(case_id, reason):
    """The condition that actually skipped this fixture in the pinned Linux run."""
    method = case_id.rsplit('.', 1)[-1]
    source = 'Lib/test/test_pathlib/test_pathlib.py'
    if reason == 'test requires a Windows-compatible system':
        return 'os.name != "nt" on the concrete WindowsPathTest fixture', source + ':3632'
    if reason == 'requires Windows-flavoured path class':
        return 'method registered by needs_windows and fixture cls.parser is posixpath', source + ':120'
    if reason == 'requires POSIX-flavoured path class':
        return 'method registered by needs_posix and fixture cls.parser is not posixpath', source + ':118'
    if reason == 'path parser is supported':
        return 'fixture cls.parser is os.path', source + ':1217'
    if reason == 'no non-broken extended attribute support':
        return 'os_helper.can_xattr() returned false for the reference filesystem', 'Lib/test/support/os_helper.py:257'
    if reason == 'test needs root privilege':
        line = {'test_owner_no_follow_symlinks': 2036, 'test_group_no_follow_symlinks': 2071}[method]
        return 'root_in_posix is false (reference effective uid is nonzero)', f'{source}:{line}'
    if reason == 'needs local target':
        line = {'test_copy_file_to_directory': 55, 'test_copy_dir_to_existing_directory': 115}[method]
        return 'isinstance(self.target_root, WritableZipPath)', f'Lib/test/test_pathlib/test_copy.py:{line}'
    if reason == 'Bad file descriptor in /dev/fd affects only macOS':
        return 'sys.platform != "darwin"', source + ':3283'
    for name, line in (('link', 2295), ('readlink', 1982), ('symlink', 2528)):
        if reason == f'os.{name}() is present':
            return f'hasattr(os, "{name}")', f'{source}:{line}'
    raise ValueError(f'undocumented reference skip: {case_id}: {reason}')


def exceptions(reference, level):
    result = {}
    for (case_id, occurrence), outcome in reference_cases(reference).items():
        if not isinstance(outcome, dict) or 'skipped' not in outcome:
            continue
        reason = outcome['skipped']
        condition, source = skip_source(case_id, reason)
        result[case_id, occurrence, level] = {
            'kind': 'upstream-skip', 'reason': reason, 'condition': condition, 'source': source,
            'configuration': 'CPython 3.14.5 regrtest -u all -X warn_default_encoding',
            'platform': reference['platform'], 'fixture_variant': case_id.rsplit('.', 1)[0]}
    return result


def lexical_adapters(tools, work):
    source, hashes = native_lexical.native_source(dispatch=True)
    binary = tools.build(source, work)
    expected = json.loads((HERE / 'lexical-results.json').read_bytes())
    if set(hashes) != set(expected['passed']):
        raise ValueError('lexical dispatch differs from pinned native methods')
    adapters = {}
    for (module, owner, fixture, ctor, _), methods in native_lexical.groups().items():
        def run(case, original, ctor=ctor):
            case_id = case.id()
            row = expected['passed'][case_id]
            if case.cls.__name__ != ctor or hashes[case_id]['method_sha256'] != row['method_sha256']:
                raise ValueError(f'wrong fixture or upstream definition: {case_id}')
            if hasattr(case, 'sep') and case.sep != ('\\' if ctor == 'PureWindowsPath' else '/'):
                raise ValueError(f'wrong upstream fixture separator: {case_id}')
            completed = subprocess.run([binary, case_id], capture_output=True, text=True, timeout=60)
            if (completed.returncode or completed.stderr
                    or completed.stdout != f"{case_id} {row['assertions']}\n"):
                raise AssertionError((case_id, completed.returncode, completed.stdout, completed.stderr))
            return {'roles': roles(body='imre', assertions='imre', operation='imre', fixture='mixed'),
                    'evidence': {'operation': f'{ctor} lexical operations in {case._testMethodName}',
                                 'fixture': 'original CPython fixture class and hooks; typed native receiver and assertions',
                                 'opt_level': tools.level, **row}}
        adapters.update({f'test.test_pathlib.{module}.{fixture}.{m}': run for m in methods})
    return adapters


def file_adapters(tools, work):
    import native_files as files

    def wrap(callback, name, native_body):
        def run(case, original):
            value = callback(case, original)
            return {'roles': roles(body='imre' if native_body else 'cpython',
                                   assertions='imre' if native_body else 'cpython',
                                   operation='imre', fixture='mixed' if native_body else 'cpython'),
                    'evidence': {'operation': 'Path.' + name.removeprefix('test_').removesuffix('_with_newlines'),
                                 'fixture': value['fixture_execution'], 'opt_level': tools.level,
                                 **{k: v for k, v in value.items() if k in ('method_sha256', 'native_calls')}}}
        return run

    adapters = {}
    for name, equality in (('test_read_bytes', 'bytes'), ('test_read_text', 'str'),
                           ('test_read_text_with_newlines', 'str')):
        adapters[files.READ + name] = wrap(files.native_read(tools, work, name, equality), name, True)
    binary = tools.build(files.DRIVER, work)
    for name in ('test_write_bytes', 'test_write_text', 'test_write_text_with_newlines'):
        adapters[files.WRITE + name] = wrap(files.host_write(binary, work / 'payload', name), name, False)
    return adapters


def main():
    cohorts = {'lexical': lexical_adapters, 'files': file_adapters, 'traversal': traversal_adapters,
               'components': component_adapters, 'walk': walk_adapters, 'basics': basic_adapters,
               'metadata': metadata_adapters, 'stat': stat_adapters, 'segments': segment_adapters,
               'samefile': samefile_adapters}
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ('imre', 'runtime', 'clang', 'result'):
        parser.add_argument('--' + option, required=True, type=Path)
    parser.add_argument('--cpython-source', type=Path)
    parser.add_argument('--imre-commit', help='full source commit used to build both supplied native artifacts')
    parser.add_argument('--body-report', type=Path,
                        help='use the complete O0/O3 whole-method probe from the same compiler for remaining IDs')
    parser.add_argument('--cohort', choices=cohorts, action='append')
    parser.add_argument('--fixture-dir', type=Path, help='parent for the original filesystem fixtures; binaries use the normal temporary directory')
    args = parser.parse_args()
    if sys.version_info[:3] != (3, 14, 5) or sys.platform != 'linux':
        parser.error('use the test-enabled CPython 3.14.5 on Linux')
    if args.cpython_source:
        sys.path.insert(0, str(args.cpython_source.resolve() / 'Lib'))
    manifest = json.loads((BENCHMARK / 'UPSTREAM.json').read_bytes())
    verify_interpreter_tests({'files': {k: v for k, v in manifest['files'].items()
                                       if k.startswith('Lib/test/test_pathlib/')}})
    reference = json.loads((HERE / 'reference-linux.json').read_bytes())
    if reference['cpython_commit'] != verify_upstream():
        raise ValueError('reference release does not match vendored tests')
    imre, runtime, clang = (p.resolve() for p in (args.imre, args.runtime, args.clang))
    body_report = json.loads(args.body_report.read_bytes()) if args.body_report else None
    if args.imre_commit and not re.fullmatch(r'[0-9a-f]{40}', args.imre_commit):
        parser.error('--imre-commit requires a full lowercase Git object ID')
    if body_report and body_report['imre_commit'] != args.imre_commit:
        parser.error('--body-report requires a matching --imre-commit for the supplied compiler and runtime')
    reports = []
    # CPython resolves PathTest.base against cwd when the original module is imported.
    with (tempfile.TemporaryDirectory(prefix='pl-build-') as tmp,
          tempfile.TemporaryDirectory(prefix='pl-fixture-', dir=args.fixture_dir) as fixture, chdir(fixture)):
        for level in (0, 3):
            tools = Toolchain(imre, runtime, clang, level)
            adapters = {}
            for cohort in (args.cohort or cohorts):
                work = Path(tmp) / f'{cohort}-{level}'
                work.mkdir()
                selected = cohorts[cohort](tools, work)
                if adapters.keys() & selected.keys():
                    raise ValueError(f'duplicate method in {cohort} cohort')
                adapters.update(selected)
            preflight = report_blockers(body_report, reference, level, adapters) if body_report else None
            report = run_evidence(load_suite('test.test_pathlib'), adapters, reference, level, preflight=preflight,
                                  exceptions=exceptions(reference, level))
            if args.imre_commit:
                for row in report['records']:
                    if row['outcome'] == 'pass':
                        row['evidence']['imre_commit'] = args.imre_commit
            print(f"O{level}: {report['counts']}", flush=True)
            reports.append(report)
    write_reports(args.result, reports, reference)
    failures = [r for report in reports for r in report['records']
                if r['outcome'] not in ('pass', 'not-run')
                and not (r['outcome'] == 'blocked' and 'whole_method_probe' in r['evidence'])]
    if failures:
        raise RuntimeError([(r['id'], r.get('reason')) for r in failures])


if __name__ == '__main__':
    main()
