"""Check native-path evidence offline without Git, a checkout, or native engines.

Place this script beside the evidence archive, manifest and logical index. All
oracles and audit helpers come from hash-verified archived source. This checks
recorded outputs/statistics and source identities; it does not remeasure speed.
"""
import argparse
import collections
from datetime import datetime
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path, PurePosixPath
import re
import runpy
import sys
import tarfile
import tempfile
from types import SimpleNamespace

if sys.flags.optimize:
    raise SystemExit('replay requires assertions; do not use -O')
sys.dont_write_bytecode = True
PREFIX = '2026-10-21-native-paths'
REVISION = 'cec52a5a963343f974ac774db5e9f25a7e7fc5ba'
ENGINES = ('relar', 'port', 'original', 'rust')
COMPUTE_ENGINES = ('relar', 'original', 'rust')


def sha(data):
    return hashlib.sha256(data).hexdigest()


def git_id(kind, data):
    return hashlib.sha1(kind.encode() + b' ' + str(len(data)).encode() + b'\0' + data).hexdigest()


def safe_name(name):
    path = PurePosixPath(name)
    assert not path.is_absolute() and '..' not in path.parts and str(path) == name and name != '.'
    return path


def native_kind(header):
    if header.startswith((b'\x7fELF', b'!<arch>\n', b'!<thin>\n', b'BC\xc0\xde', b'\xde\xc0\x17\x0b', b'MZ', b'\0asm')):
        return True
    if header[:4] in {b'\xfe\xed\xfa\xce', b'\xce\xfa\xed\xfe', b'\xfe\xed\xfa\xcf', b'\xcf\xfa\xed\xfe',
                      b'\xca\xfe\xba\xbe', b'\xbe\xba\xfe\xca', b'\xca\xfe\xba\xbf', b'\xbf\xba\xfe\xca'}:
        return True
    magic = int.from_bytes(header[:2], 'little')
    return header[:4] == importlib.util.MAGIC_NUMBER or header[2:4] == b'\r\n' and (3000 <= magic < 4000 or magic == 62211)


def source_tree_id(entries):
    root = {}
    for name, item in entries.items():
        parts = safe_name(name).parts
        current = root
        for part in parts[:-1]:
            current = current.setdefault(part, {})
            assert isinstance(current, dict)
        assert parts[-1] not in current
        current[parts[-1]] = (item['mode'], item['git_blob'])
    def tree(node):
        result = bytearray()
        for name, item in sorted(node.items(), key=lambda pair: (pair[0] + ('/' if isinstance(pair[1], dict) else '')).encode()):
            if isinstance(item, dict):
                mode, value = '40000', tree(item)
            else:
                mode, value = format(0o100000 | item[0], 'o'), item[1]
            result.extend(mode.encode() + b' ' + name.encode() + b'\0' + bytes.fromhex(value))
        return git_id('tree', bytes(result))
    return tree(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, default=Path(__file__).with_name(PREFIX))
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    prefix = args.prefix.absolute()
    manifest_data = Path(str(prefix) + '-evidence-manifest.json').read_bytes()
    manifest = json.loads(manifest_data)
    assert manifest['status'] == 'passed' and manifest['measurement_revision'] == REVISION
    assert manifest['replay_sha256'] == sha(Path(__file__).read_bytes()), 'replay script differs'
    archive_path = prefix.parent / manifest['archive']['name']
    assert archive_path.name == PREFIX + '-evidence.tar.gz'
    archive_bytes = archive_path.read_bytes()
    assert len(archive_bytes) == manifest['archive']['bytes'] and sha(archive_bytes) == manifest['archive']['sha256']
    index_data = Path(str(prefix) + '-evidence-index.json').read_bytes()
    assert sha(index_data) == manifest['logical_index_sha256']
    index = json.loads(index_data)
    assert index['source_roles'] == manifest['source_roles'] == {'measurement': REVISION}
    payloads = {}
    with tarfile.open(archive_path, 'r:gz') as archive:
        for member in archive:
            safe_name(member.name)
            assert member.isfile() and not member.issym() and not member.islnk() and member.name not in payloads
            assert member.mtime == member.uid == member.gid == 0 and member.uname == member.gname == ''
            data = archive.extractfile(member).read()
            expected = manifest['members'][member.name]
            assert len(data) == expected['bytes'] and sha(data) == expected['sha256'] and member.mode == expected['mode']
            assert not native_kind(data[:16]), member.name
            payloads[member.name] = data
    assert set(payloads) == set(manifest['members']) and len(payloads) == manifest['member_count']
    assert payloads['provenance/logical-index.json'] == index_data
    packaged_metadata = json.loads(payloads['provenance/package.json'])
    assert all(manifest[key] == value for key, value in packaged_metadata.items())
    exclusions = json.loads(payloads['provenance/exclusions.json'])
    assert len(exclusions) == len(index['excluded']) == manifest['excluded_artifact_count']
    for item in exclusions:
        assert {key: item[key] for key in ('sha256', 'bytes', 'reason')} == index['excluded'][item['path']]

    def blob(item, expected=None):
        data = payloads[item['member']]
        assert len(data) == item['bytes'] and sha(data) == item['sha256']
        assert expected is None or item['sha256'] == expected
        return data

    root = Path(manifest['repository'])
    source = index['sources'][REVISION]
    assert len(source) == manifest['logical_source_count']
    assert not any(name.startswith('benchmarks/results/') for name in source)
    omitted = index['omitted_historical_results'][REVISION]
    assert all(name.startswith('benchmarks/results/') for name in omitted) and not (source.keys() & omitted.keys())
    for name, item in source.items():
        safe_name(name)
        assert item['mode'] in (0o644, 0o755) and git_id('blob', blob(item)) == item['git_blob']
    tree_id = source_tree_id({**source, **omitted})
    assert tree_id == index['source_trees'][REVISION]
    commit = blob(index['source_commits'][REVISION])
    assert git_id('commit', commit) == REVISION and commit.splitlines()[0] == b'tree ' + tree_id.encode()
    assert len(index['files']) == manifest['logical_file_count']
    for path, item in index['files'].items():
        assert Path(path).is_absolute()
        blob(item)
    for item in index['captured_source_bindings']:
        assert item['revision'] == REVISION and str(root / item['relative']) == item['path']
        assert all(item[key] == source[item['relative']][key] for key in ('member', 'sha256', 'bytes', 'mode', 'git_blob'))
    for directory, entries in index['trees'].items():
        assert Path(directory).is_absolute()
        for name, is_directory in entries.items():
            safe_name(name)
            assert isinstance(is_directory, bool)
            path = str(Path(directory) / name)
            if not is_directory:
                assert path in index['files'] or path in index['excluded']

    def identity(path):
        name = str(path)
        if name in index['files']:
            return index['files'][name]
        if name in index['excluded']:
            return index['excluded'][name]
        return source[Path(path).relative_to(root).as_posix()]

    def retained(path, expected=None):
        item = identity(path)
        assert 'member' in item, ('native artifact intentionally excluded', path)
        return blob(item, expected)

    def load(path, expected=None):
        return json.loads(retained(path, expected))

    audit = load(manifest['audit']['path'], manifest['audit']['sha256'])
    assert audit['status'] == 'passed' and audit['measurement_revision'] == REVISION
    assert audit['audit_script_sha256'] == manifest['audit_script']['sha256']
    for path, expected in audit['guarded_inputs'].items():
        assert identity(path)['sha256'] == expected, ('audit identity', path)
    build = load(manifest['build']['path'], manifest['build']['sha256'])
    assert build['revision'] == REVISION and build['source_matches_revision'] and build['input_guards_passed']
    assert all(source[name]['sha256'] == value for name, value in build['source_sha256'].items())
    build_root = Path(manifest['build']['path']).parent
    for key, name in {'compiler': 'relar', 'normal_runtime': 'normal_runtime.a', 'thinlto_runtime': 'thinlto_runtime.a', 'asan_runtime': 'asan_runtime.a'}.items():
        assert identity(build_root / name)['sha256'] == build['sha256'][key]

    validation_root = Path(manifest['language_validation_root'])
    normal = load(validation_root / 'normal/summary.json')
    assert normal['revision'] == REVISION and normal['returncode'] == 0 and normal['failed_suites'] == []
    assert normal['suite_count'] == normal['passed_suite_count'] == len(normal['suites']) == 32
    assert normal['known_reported_check_total'] == sum(item['reported_check_count'] or 0 for item in normal['suites']) == 1883
    assert normal['compiler_runtime_hashes_unchanged'] and normal['source_hashes_match_record']
    assert normal['hashes_before'] == normal['hashes_after'] == {key: build['sha256'][key] for key in ('compiler', 'normal_runtime')}
    assert normal['source_hash_count'] == len(build['source_sha256']) == 70
    assert normal['build_record_sha256'] == manifest['build']['sha256'] and normal['optimization_levels'] == [0, 3]
    assert source['.github/workflows/ci.yml']['sha256'] == normal['ci_sha256']
    ci = blob(source['.github/workflows/ci.yml']).decode()
    ordinary = ci.split('- name: Compare native suites with CPython', 1)[1].split('- name:', 1)[0]
    expected_suites = re.search(r'--suites (.*?) \\\n', ordinary).group(1).split()
    assert [item['suite'] for item in normal['suites']] == expected_suites
    runner = retained(normal['runner_log'], normal['runner_log_sha256']).decode()
    for item in normal['suites']:
        log = retained(item['log'], item['log_sha256']).decode()
        assert item['passed'] and re.search(r'^PASS ' + re.escape(item['suite']) + r' \(', runner, re.M)
        assert all(line in log for line in item['reported_summary'])
    affected = load(validation_root / 'affected/record.json')
    assert affected['revision'] == REVISION and affected['input_guards_passed']
    for mode in ('thinlto', 'asan-unsandboxed'):
        run = next(row for row in affected['runs'] if row['mode'] == mode)
        assert run['exit_code'] == 0
        command = run['command']
        suites = command[command.index('--suites') + 1:command.index('--')]
        assert len(suites) == 7
        log = retained(validation_root / 'affected' / (mode + '.log')).decode()
        assert all(re.search(r'^PASS ' + re.escape(suite) + r' \(', log, re.M) for suite in suites)
    paths = load(validation_root / 'paths/record.json')
    assert paths['revision'] == REVISION and paths['sources_unchanged']
    assert paths['before_sha256'] == paths['after_sha256'] == build['sha256']
    assert all(source[name]['sha256'] == value for name, value in paths['suite_sha256'].items())
    assert set(paths['results']) == {'thinlto-o0-o3', 'asan-o0-o1'}
    for row in paths['results'].values():
        assert row['returncode'] == 0 and row['checks'] == 67
        assert 'Passed 67 ' in retained(row['log']).decode()

    context = load('/tmp/relar-path-timing-context.json')
    assert context['revision'] == REVISION and context['status'] == 'passed' and context['cpu'] == 31
    assert [row['mode'] for row in context['windows']] == ['normal', 'thinlto']
    assert all(row['exit_code'] == 0 for row in context['windows'])
    assert context['commands'] == load('/tmp/relar-path-timing-commands.json')
    for row in context['windows']:
        assert row['command'] == context['commands'][row['mode']]
        retained(row['log'])
    counts, modes = collections.Counter(), {}
    with tempfile.TemporaryDirectory(prefix='native-paths-portable-replay-') as temporary:
        extracted = Path(temporary) / 'source'
        for name, item in source.items():
            path = extracted / safe_name(name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(blob(item))
        audit_file = Path(temporary) / 'audit.py'
        audit_file.write_bytes(retained(manifest['audit_script']['path'], manifest['audit_script']['sha256']))
        helpers = runpy.run_path(str(audit_file))
        helpers['original_commands'].__globals__['ROOT'] = root
        benchmark = runpy.run_path(str(extracted / 'benchmarks/memory_benchmark.py'))
        validator = benchmark['validator']
        check_capture, check_row = helpers['check_capture'], helpers['check_row']
        cases = validator.cases()
        assert len(cases) == 92
        selected = {'directory-common-projects', 'directory-dotfile-unicode-and-multiple-suffix',
                    'directory-stable-project-ties', 'directory-big-integers', 'directory-unchanged', 'directory-ignored-schema-fields'}
        compute_cases = [validator.compute_case(case, rounds) for case, error, original in cases if case.label in selected for rounds in (0, 2)]
        assert len(compute_cases) == 12

        def check_validation(ordinary, computation, label):
            assert ordinary['cases'] == len(ordinary['results']) == 92 and ordinary['engine_executions'] == 335
            assert set(ordinary['engines']) == set(computation['engines']) == set(ENGINES)
            for capture, (case, error, original) in zip(ordinary['results'], cases, strict=True):
                assert capture['case'] == validator.case_record(case) and capture['expected_error'] == error
                assert set(capture['engines']) == set(ENGINES) - (set() if original else {'original'})
                for engine, result in capture['engines'].items():
                    check_capture(validator, case, validator.oracle(case, error), result, engine)
                    counts[label + '_application_executions'] += 1
            assert computation['cases'] == len(computation['results']) == 12 and computation['engine_executions'] == 48
            for capture, case in zip(computation['results'], compute_cases, strict=True):
                assert capture['case'] == validator.case_record(case) and set(capture['engines']) == set(ENGINES)
                for engine, result in capture['engines'].items():
                    check_capture(validator, case, validator.compute_oracle(case), result, engine, compute=True)
                    counts[label + '_compute_executions'] += 1

        empty = validator.case('empty-control', validator.report(), validator.report())
        descriptions = [(benchmark['describe'](empty, 0, 'empty-schema-valid-reports'), empty),
                        *benchmark['workloads']([32, 256], [(256, 1), (4, 64)])]
        assert len(descriptions) == 11
        previous = datetime.fromisoformat(context['started_at'])
        for mode in ('normal', 'thinlto'):
            timing = load(manifest['timings'][mode]['path'], manifest['timings'][mode]['sha256'])
            prepared = load(manifest['preparations'][mode]['path'], manifest['preparations'][mode]['sha256'])
            assert timing['preparation'] == prepared and timing['preparation_record_sha256'] == manifest['preparations'][mode]['sha256']
            assert timing['revision'] == prepared['revision'] == REVISION and not timing['dirty'] and not prepared['dirty']
            assert timing['input_guards_passed'] and prepared['input_guards_passed'] and prepared['thinlto'] == (mode == 'thinlto')
            assert timing['samples'] == 12 and timing['warmups'] == 2 and timing['copies'] == [32, 256]
            assert timing['directory_layouts'] == [[256, 1], [4, 64]] and timing['compute_rounds'] == 8
            assert timing['cpu']['affinity'] == [31] and timing['python']['version_number'] == '3.14.5'
            started, completed = (datetime.fromisoformat(timing[key]) for key in ('started_at_utc', 'completed_at_utc'))
            assert previous <= started < completed <= datetime.fromisoformat(context['finished_at'])
            assert datetime.fromisoformat(prepared['completed_at_utc']) <= started
            previous = completed
            captured_paths = prepared['source_and_tool_paths']
            assert captured_paths.keys() == prepared['source_and_tool_sha256'].keys()
            for name, path in captured_paths.items():
                assert identity(path)['sha256'] == prepared['source_and_tool_sha256'][name]
            assert prepared['source_and_tool_sha256']['compiler'] == build['sha256']['compiler']
            assert prepared['source_and_tool_sha256']['runtime'] == build['sha256'][mode + '_runtime']
            for name in ('binary', 'llvm', 'validation', 'compute_binary', 'compute_llvm', 'compute_validation'):
                assert identity(prepared[name])['sha256'] == prepared[name + '_sha256']
            rust = load(captured_paths['rust_build_record'])
            assert rust == prepared['rust_build_record'] and rust['input_guards_passed'] and rust['thinlto'] == (mode == 'thinlto')
            assert rust['binary_sha256'] == prepared['source_and_tool_sha256']['rust'] and rust['rustc_verbose'].strip() == build['toolchain'].strip()
            for name, value in rust['sources'].items():
                data = blob(source['benchmarks/reference/memory_report/' + name])
                assert sha(data) == value and rust['source_contents'][name] == data.decode()
            check_validation(load(prepared['validation']), load(prepared['compute_validation']), 'preparation')
            commands = helpers['original_commands'](benchmark, prepared, captured_paths, extracted)
            compute_commands = helpers['original_commands'](benchmark, prepared, captured_paths, extracted, compute=True)
            rows = [timing['startup_control'], *timing['workloads']]
            application = []
            flat_rows = list(rows)
            for row, (description, case) in zip(rows, descriptions, strict=True):
                result = check_row(validator, row, description, case, commands, samples=12, warmups=2)
                counts.update(result['counts'])
                if application:
                    fractions = {engine: application[0]['seconds'][engine] / result['seconds'][engine] for engine in ENGINES}
                    assert row['startup_control_fraction'] == fractions
                    result['startup_control_fraction'] = fractions
                application.append(result)
            full = [(description, case) for description, case in descriptions if description.get('mode') == 'full']
            assert len(full) == len(timing['compute_workloads']) == 2
            computation = []
            for row, (description, ordinary) in zip(timing['compute_workloads'], full, strict=True):
                case = validator.compute_case(ordinary, 8)
                metadata = {**description, 'label': case.label, 'arguments': list(case.arguments), 'rounds': 8}
                result = check_row(validator, row, metadata, case, compute_commands, samples=12, warmups=2, compute=True)
                control = validator.compute_case(ordinary, 0)
                metadata = {**description, 'label': control.label, 'arguments': list(control.arguments), 'rounds': 0,
                            'expected_markdown_sha256': sha(b''), 'expected_markdown_bytes': 0}
                preload = check_row(validator, row['preload_control'], metadata, control, compute_commands, samples=12, warmups=2, compute=True)
                counts.update(result['counts']); counts.update(preload['counts'])
                amortized = {engine: seconds / 8 for engine, seconds in result['seconds'].items()}
                fractions = {engine: preload['seconds'][engine] / result['seconds'][engine] for engine in COMPUTE_ENGINES}
                assert row['amortized_seconds'] == amortized and row['preload_control_fraction'] == fractions
                result.update(amortized_seconds=amortized, preload_control_fraction=fractions, preload_control=preload)
                computation.append(result)
                flat_rows.extend([row, row['preload_control']])
            modes[mode] = {'application': application, 'compute': computation}
            assert modes[mode] == audit['modes'][mode]
            directory = Path(manifest['timings'][mode]['path']).with_suffix('')
            entries = index['trees'][str(directory)]
            assert {PurePosixPath(name).parts[0] for name in entries} == {row['label'] for row in flat_rows}
            for row in flat_rows:
                assert {PurePosixPath(name).parts[1] for name in entries if name.startswith(row['label'] + '/') and len(PurePosixPath(name).parts) >= 2} == set(row['gates'])
                for engine, capture in row['gates'].items():
                    relative = row['label'] + '/' + engine + '/'
                    actual = {name[len(relative):]: retained(directory / name) for name, is_directory in entries.items()
                              if name.startswith(relative) and not is_directory}
                    directories = sorted(name[len(relative):] for name, is_directory in entries.items() if name.startswith(relative) and is_directory)
                    assert helpers['encoded'](actual) == capture['files'] and directories == capture['directories']
            sidecar = Path(str(prefix) + '-' + mode + '.json.gz')
            if sidecar.exists():
                assert gzip.decompress(sidecar.read_bytes()) == retained(manifest['timings'][mode]['path'])
        assert {key: counts[key] for key in audit['counts']} == audit['counts']
        asan_prep = load(manifest['asan_application']['preparation']['path'], manifest['asan_application']['preparation']['sha256'])
        assert asan_prep['revision'] == REVISION and asan_prep['passed'] and asan_prep['steps'][-1]['returncode'] == 0
        for item in asan_prep['artifacts'].values():
            assert identity(item['path'])['sha256'] == item['sha256']
        for step in asan_prep['steps']:
            retained(step['log'], step['log_sha256'])
        asan = load(manifest['asan_application']['validation']['path'], manifest['asan_application']['validation']['sha256'])
        check_validation(asan, asan['compute'], 'asan')
    result = {'schema': 1, 'status': 'passed', 'measurement_revision': REVISION,
              'archive_sha256': manifest['archive']['sha256'], 'manifest_sha256': sha(manifest_data),
              'source_tree': tree_id, 'source_files': len(source), 'omitted_historical_result_files': len(omitted),
              'retained_logical_files': len(index['files']), 'excluded_native_artifacts': len(index['excluded']),
              'normal_suites': 32, 'normal_reported_checks': 1883, 'affected_suites_per_mode': 7,
              'path_checks_per_mode': 67, 'counts': dict(counts), 'modes': modes,
              'limits': 'Data-only replay. Hashes bind archived source, captures and recorded external artifact identities; absent native tools/binaries cannot be rehashed here. No native execution, timing remeasurement, timestamp authentication, or proof of historical host isolation. Repeated-compute timings retain preload/startup/final-output costs and subtract no controls.'}
    data = (json.dumps(result, indent=2) + '\n').encode()
    if args.output:
        with args.output.open('xb') as stream:
            stream.write(data)
    print(json.dumps({key: value for key, value in result.items() if key != 'modes'}, indent=2))


if __name__ == '__main__':
    main()
